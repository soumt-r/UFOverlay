//! Zero-copy capture through Windows.Graphics.Capture (monitors *and* windows).
//!
//! DWM hands us GPU frames on our own D3D11 device; we copy the newest one into
//! a composite texture  that SteamVR samples. Frames
//! arrive by event, so an unchanged source costs nothing. `MinUpdateInterval`
//! asks DWM itself to rate-limit, so a 240 Hz desktop is never copied 240×/s.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::{IInspectable, Interface};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::SetEvent;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use crate::sources::Source;

pub struct Gpu {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
    /// Auto-reset event signalled whenever any capture has a new frame.
    pub wake: HANDLE,
}

impl Gpu {
    /// D3D11 device on SteamVR's adapter, so SetOverlayTexture is a same-GPU share.
    pub fn new(vr_adapter: i32, wake: HANDLE) -> windows::core::Result<Self> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let adapter = unsafe { factory.EnumAdapters1(vr_adapter.max(0) as u32) }.or_else(|_| unsafe { factory.EnumAdapters1(0) })?;
        let mut device = None;
        let mut ctx = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
        }
        let device: ID3D11Device = device.unwrap();
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        Ok(Self { device, ctx: ctx.unwrap(), winrt, wake })
    }

    /// RGBA texture for CPU-drawn UI (straight from a Canvas, no swizzle).
    pub fn ui_texture(&self, w: u32, h: u32) -> windows::core::Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        Ok(tex.unwrap())
    }

    fn texture(&self, w: u32, h: u32) -> windows::core::Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        Ok(tex.unwrap())
    }
}

fn item_for(src: &Source) -> windows::core::Result<GraphicsCaptureItem> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe {
        match src {
            Source::Monitor { hmon, .. } => interop.CreateForMonitor(*hmon),
            Source::Window { hwnd, .. } => interop.CreateForWindow(*hwnd),
        }
    }
}

const FMT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

pub struct Capture {
    item: GraphicsCaptureItem,
    pool: Option<Direct3D11CaptureFramePool>,
    session: Option<GraphicsCaptureSession>,
    pending: Arc<AtomicBool>,
    pub composite: ID3D11Texture2D,
    pub w: u32,
    pub h: u32,
    min_interval_100ns: i64,
}

pub enum Grab {
    None,
    Frame,
    /// Content size changed; composite was recreated.
    Resized,
}

impl Capture {
    pub fn new(gpu: &Gpu, src: &Source) -> windows::core::Result<Self> {
        let item = item_for(src)?;
        let size = item.Size()?;
        let (w, h) = (size.Width.max(1) as u32, size.Height.max(1) as u32);
        let composite = gpu.texture(w, h)?;
        Ok(Self { item, pool: None, session: None, pending: Arc::new(AtomicBool::new(false)), composite, w, h, min_interval_100ns: 0 })
    }

    pub fn tex_ptr(&self) -> *mut c_void {
        self.composite.as_raw()
    }

    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    pub fn start(&mut self, gpu: &Gpu, fps: u32) -> windows::core::Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let size = SizeInt32 { Width: self.w as i32, Height: self.h as i32 };
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.winrt, FMT, 2, size)?;
        let pending = self.pending.clone();
        let wake = gpu.wake.0 as usize;
        pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
            pending.store(true, Ordering::Relaxed);
            unsafe {
                let _ = SetEvent(HANDLE(wake as *mut c_void));
            }
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&self.item)?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        let _ = session.SetIsBorderRequired(false);
        self.pool = Some(pool);
        self.session = Some(session);
        self.min_interval_100ns = 0;
        self.set_rate(fps);
        self.session.as_ref().unwrap().StartCapture()?;
        Ok(())
    }

    /// Ask DWM not to deliver faster than `fps` (Windows 11 24H2+; ignored before).
    pub fn set_rate(&mut self, fps: u32) {
        let iv = 10_000_000 / fps.max(1) as i64;
        if iv != self.min_interval_100ns {
            if let Some(s) = &self.session {
                let _ = s.SetMinUpdateInterval(TimeSpan { Duration: iv });
            }
            self.min_interval_100ns = iv;
        }
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.session.take() {
            let _ = s.Close();
        }
        if let Some(p) = self.pool.take() {
            let _ = p.Close();
        }
        self.pending.store(false, Ordering::Relaxed);
    }

    /// Copy the newest available frame (if any) into the composite.
    pub fn grab(&mut self, gpu: &Gpu) -> windows::core::Result<Grab> {
        if !self.pending.swap(false, Ordering::Relaxed) {
            return Ok(Grab::None);
        }
        let Some(pool) = &self.pool else { return Ok(Grab::None) };
        // Drain: only the newest frame matters.
        let mut latest = None;
        while let Ok(f) = pool.TryGetNextFrame() {
            if let Some(old) = latest.replace(f) {
                let _ = old.Close();
            }
        }
        let Some(frame) = latest else { return Ok(Grab::None) };
        let cs = frame.ContentSize()?;
        let (cw, ch) = (cs.Width.max(1) as u32, cs.Height.max(1) as u32);
        let mut out = Grab::Frame;
        if cw != self.w || ch != self.h {
            self.w = cw;
            self.h = ch;
            self.composite = gpu.texture(cw, ch)?;
            pool.Recreate(&gpu.winrt, FMT, 2, cs)?;
            out = Grab::Resized;
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let tex: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        let b = D3D11_BOX { left: 0, top: 0, front: 0, right: self.w.min(desc.Width), bottom: self.h.min(desc.Height), back: 1 };
        unsafe { gpu.ctx.CopySubresourceRegion(&self.composite, 0, 0, 0, 0, &tex, 0, Some(&b)) };
        let _ = frame.Close();
        Ok(out)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}
