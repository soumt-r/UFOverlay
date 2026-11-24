//! Embeds assets/ufoverlay.ico as the exe icon without rc.exe or any crate:
//! we write the compiled-resource (.res) format by hand and hand it to the
//! MSVC linker, which accepts .res files as plain inputs.

use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn align4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

/// One RESOURCEHEADER (numeric type + numeric name) followed by its data.
fn resource(out: &mut Vec<u8>, ty: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&name.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&flags.to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // LanguageId (en-US)
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    align4(out);
}

fn main() {
    println!("cargo:rerun-if-changed=assets/ufoverlay.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let ico = std::fs::read("assets/ufoverlay.ico").expect("assets/ufoverlay.ico");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;

    // The empty resource every .res file starts with.
    let mut res = vec![0, 0, 0, 0, 0x20, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0];
    res.extend_from_slice(&[0; 16]);

    // RT_ICON per image, then a GRPICONDIR pointing at them by id.
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let e = &ico[6 + 16 * i..22 + 16 * i];
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let id = (i + 1) as u16;
        resource(&mut res, RT_ICON, id, 0x1010, &ico[off..off + size]);
        group.extend_from_slice(&e[..12]); // w, h, colors, reserved, planes, bpp, bytes
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ufoverlay.res");
    std::fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
