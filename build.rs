//! Embeds `assets/icon.ico` as the executable's icon, which Explorer shows and the tray loads.
//! The icon file is packed straight into a compiled resource (`.res`) file that the linker takes
//! as an input, so no resource compiler is needed.
//!
//! `assets/icon.ico` is rendered from `assets/icon.svg`: 32-bit BMP images from 16 to 48 px, PNG
//! images from 60 to 256 px. Re-render it after editing the SVG.

use std::env;
use std::fs;
use std::path::PathBuf;

/// The icon group's resource ID. The tray loads the icon by this ID.
const ICON_ID: u16 = 1;
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// Memory flags `rc.exe` gives icons. Windows ignores them, but they keep the file conventional.
const MOVEABLE_DISCARDABLE: u16 = 0x1010;

fn main() {
    println!("cargo::rerun-if-changed=assets/icon.ico");
    // link.exe and lld-link accept a .res file as an input; GNU ld does not.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let ico = fs::read("assets/icon.ico").expect("read assets/icon.ico");
    let res = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("icon.res");
    fs::write(&res, icon_res(&ico)).expect("write icon.res");
    println!("cargo::rustc-link-arg-bins={}", res.display());
}

/// Converts an `.ico` file into a resource file: one `RT_ICON` per image, plus the
/// `RT_GROUP_ICON` directory that lists them by ID.
fn icon_res(ico: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);
    assert!(
        u16_at(0) == 0 && u16_at(2) == 1,
        "assets/icon.ico is not an icon file"
    );
    let count = u16_at(4);

    // A resource file starts with an empty entry.
    let mut res = Vec::new();
    resource(&mut res, 0, 0, &[]);
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&count.to_le_bytes());
    for id in 1..=count {
        let entry = 6 + 16 * usize::from(id - 1);
        let size = u32_at(entry + 8) as usize;
        let offset = u32_at(entry + 12) as usize;
        resource(&mut res, RT_ICON, id, &ico[offset..offset + size]);
        // A group entry is the file's directory entry with the file offset replaced by the ID.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, ICON_ID, &group);
    res
}

/// Appends one language-neutral resource with a numeric type and name, padded to 4 bytes.
fn resource(res: &mut Vec<u8>, kind: u16, name: u16, data: &[u8]) {
    let data_size = u32::try_from(data.len()).expect("resource fits in 4 GiB");
    let header_size: u32 = 32;
    let flags = if kind == 0 { 0 } else { MOVEABLE_DISCARDABLE };
    res.extend_from_slice(&data_size.to_le_bytes());
    res.extend_from_slice(&header_size.to_le_bytes());
    for ordinal in [kind, name] {
        res.extend_from_slice(&0xFFFFu16.to_le_bytes());
        res.extend_from_slice(&ordinal.to_le_bytes());
    }
    res.extend_from_slice(&0u32.to_le_bytes()); // data version
    res.extend_from_slice(&flags.to_le_bytes());
    res.extend_from_slice(&0u16.to_le_bytes()); // language
    res.extend_from_slice(&0u32.to_le_bytes()); // version
    res.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
}
