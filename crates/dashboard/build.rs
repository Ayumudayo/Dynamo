use sha2::{Digest, Sha256};
use std::{env, fmt::Write as _, fs, path::Path};

struct FontAsset {
    const_name: &'static str,
    file: &'static str,
    route_slug: &'static str,
    family: &'static str,
    weight: &'static str,
    expected_sha256: &'static str,
}

struct LockedFile {
    file: &'static str,
    expected_sha256: &'static str,
}

struct FallbackFace {
    family: &'static str,
    local_family: &'static str,
    weight: u16,
    size_adjust: &'static str,
    ascent_override: &'static str,
    descent_override: &'static str,
}

const FONTS: &[FontAsset] = &[
    FontAsset {
        const_name: "FIRA_SANS_LIGHT",
        file: "FiraSans-Light.woff2",
        route_slug: "fira-sans-light",
        family: "Fira Sans",
        weight: "300",
        expected_sha256: "2315d21be4c62def3fa08de87f29c327187affcd7759a46596d5bacfeb2ff221",
    },
    FontAsset {
        const_name: "FIRA_SANS_REGULAR",
        file: "FiraSans-Regular.woff2",
        route_slug: "fira-sans-regular",
        family: "Fira Sans",
        weight: "400",
        expected_sha256: "51000d3cc8a601427bdb88625275e0eefc00570f4f2ab7a926fa336abee7098f",
    },
    FontAsset {
        const_name: "FIRA_SANS_MEDIUM",
        file: "FiraSans-Medium.woff2",
        route_slug: "fira-sans-medium",
        family: "Fira Sans",
        weight: "500",
        expected_sha256: "14d421d4fb35d56b40e958cc9b64eb1528d1822bb75251623955c873a9a175d3",
    },
    FontAsset {
        const_name: "FIRA_SANS_SEMIBOLD",
        file: "FiraSans-SemiBold.woff2",
        route_slug: "fira-sans-semibold",
        family: "Fira Sans",
        weight: "600",
        expected_sha256: "01ca0c4a1f02dd4324721cf8766bcbb2edca4dee0fb8884a66e58e231ed62d55",
    },
    FontAsset {
        const_name: "FIRA_SANS_BOLD",
        file: "FiraSans-Bold.woff2",
        route_slug: "fira-sans-bold",
        family: "Fira Sans",
        weight: "700",
        expected_sha256: "7dff4e2351ca54fae96180e79ab1d1fdeb74ebb6e6cba53a7d9d7b4941600ada",
    },
    FontAsset {
        const_name: "FIRA_CODE_VARIABLE",
        file: "FiraCode-Variable.woff2",
        route_slug: "fira-code",
        family: "Fira Code",
        weight: "500 700",
        expected_sha256: "408e876a202f15ea6ee307a70a65cf40ceb222c589a0b17e0a3a371db96dd49f",
    },
];

const LICENSES: &[LockedFile] = &[
    LockedFile {
        file: "OFL-FiraSans.txt",
        expected_sha256: "0f00200159149239638e56183cd73f345b202af97c4486787df148d656e52fd2",
    },
    LockedFile {
        file: "OFL-FiraCode.txt",
        expected_sha256: "1d41e10031ab125302780a05ec4c91d218e47db0c7e37cf315cce5e608cdc25c",
    },
];

const FALLBACKS: &[FallbackFace] = &[
    FallbackFace {
        family: "Fira Sans Fallback",
        local_family: "Segoe UI Light",
        weight: 300,
        size_adjust: "104.6000%",
        ascent_override: "89.3881%",
        descent_override: "25.3346%",
    },
    FallbackFace {
        family: "Fira Sans Fallback",
        local_family: "Segoe UI",
        weight: 400,
        size_adjust: "105.4000%",
        ascent_override: "88.7097%",
        descent_override: "25.1423%",
    },
    FallbackFace {
        family: "Fira Sans Fallback",
        local_family: "Segoe UI Semibold",
        weight: 500,
        size_adjust: "105.8000%",
        ascent_override: "88.3743%",
        descent_override: "25.0473%",
    },
    FallbackFace {
        family: "Fira Sans Fallback",
        local_family: "Segoe UI Semibold",
        weight: 600,
        size_adjust: "106.0000%",
        ascent_override: "88.2075%",
        descent_override: "25.0000%",
    },
    FallbackFace {
        family: "Fira Sans Fallback",
        local_family: "Segoe UI Bold",
        weight: 700,
        size_adjust: "106.2000%",
        ascent_override: "88.0414%",
        descent_override: "24.9529%",
    },
    FallbackFace {
        family: "Fira Code Fallback",
        local_family: "Consolas",
        weight: 500,
        size_adjust: "109.8376%",
        ascent_override: "84.0402%",
        descent_override: "28.0134%",
    },
    FallbackFace {
        family: "Fira Code Fallback",
        local_family: "Consolas Bold",
        weight: 600,
        size_adjust: "108.5403%",
        ascent_override: "85.0446%",
        descent_override: "28.3482%",
    },
    FallbackFace {
        family: "Fira Code Fallback",
        local_family: "Consolas Bold",
        weight: 700,
        size_adjust: "108.5403%",
        ascent_override: "85.0446%",
        descent_override: "28.3482%",
    },
];

fn sha256(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|error| {
        panic!(
            "could not read locked font file {}: {error}",
            path.display()
        )
    });
    format!("{:x}", Sha256::digest(bytes))
}

fn assert_lock_entry(lock: &str, file: &str, expected_sha256: &str) {
    assert!(
        lock.contains(&format!("\"file\": \"{file}\"")),
        "fonts.lock.json does not name {file}"
    );
    assert!(
        lock.contains(&format!("\"sha256\": \"{expected_sha256}\"")),
        "fonts.lock.json does not lock the reviewed SHA-256 for {file}"
    );
}

fn main() {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .expect("CARGO_MANIFEST_DIR is required");
    let font_dir = manifest_dir.join("assets/fonts");
    let lock_path = font_dir.join("fonts.lock.json");
    let lock = fs::read_to_string(&lock_path).unwrap_or_else(|error| {
        panic!(
            "could not read deterministic font lock {}: {error}",
            lock_path.display()
        )
    });

    println!("cargo:rerun-if-changed={}", lock_path.display());

    let mut generated = String::from("// @generated by crates/dashboard/build.rs; do not edit.\n");
    let mut font_face_css = String::new();

    for font in FONTS {
        let path = font_dir.join(font.file);
        println!("cargo:rerun-if-changed={}", path.display());
        let actual_sha256 = sha256(&path);
        assert_eq!(
            actual_sha256, font.expected_sha256,
            "reviewed font hash changed for {}",
            font.file
        );
        assert_lock_entry(&lock, font.file, font.expected_sha256);

        let route = format!("/assets/fonts/{}-{}.woff2", font.route_slug, actual_sha256);
        let etag = format!("\"{}\"", actual_sha256);
        writeln!(
            generated,
            "pub const {}_BYTES: &[u8] = include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/fonts/{}\"));",
            font.const_name, font.file
        )
        .expect("write generated font bytes constant");
        writeln!(
            generated,
            "pub const {}_PATH: &str = {:?};",
            font.const_name, route
        )
        .expect("write generated font path constant");
        writeln!(
            generated,
            "pub const {}_SHA256: &str = {:?};",
            font.const_name, actual_sha256
        )
        .expect("write generated font hash constant");
        writeln!(
            generated,
            "pub const {}_ETAG: &str = {:?};",
            font.const_name, etag
        )
        .expect("write generated font ETag constant");

        writeln!(
            font_face_css,
            "@font-face {{\n  font-family: '{}';\n  font-style: normal;\n  font-weight: {};\n  font-display: swap;\n  src: url('{}') format('woff2');\n}}",
            font.family, font.weight, route
        )
        .expect("write primary font face");
    }

    for license in LICENSES {
        let path = font_dir.join(license.file);
        println!("cargo:rerun-if-changed={}", path.display());
        let actual_sha256 = sha256(&path);
        assert_eq!(
            actual_sha256, license.expected_sha256,
            "reviewed license hash changed for {}",
            license.file
        );
        assert_lock_entry(&lock, license.file, license.expected_sha256);
    }

    for fallback in FALLBACKS {
        writeln!(
            font_face_css,
            "@font-face {{\n  font-family: '{}';\n  font-style: normal;\n  font-weight: {};\n  src: local('{}');\n  size-adjust: {};\n  ascent-override: {};\n  descent-override: {};\n  line-gap-override: 0.0000%;\n}}",
            fallback.family,
            fallback.weight,
            fallback.local_family,
            fallback.size_adjust,
            fallback.ascent_override,
            fallback.descent_override,
        )
        .expect("write fallback font face");
    }

    writeln!(
        generated,
        "pub const FONT_FACE_CSS: &str = {:?};",
        font_face_css
    )
    .expect("write generated font CSS constant");

    let out_dir = env::var_os("OUT_DIR")
        .map(std::path::PathBuf::from)
        .expect("OUT_DIR is required");
    fs::write(out_dir.join("font_assets.rs"), generated)
        .expect("write generated font asset constants");
}
