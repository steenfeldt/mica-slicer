// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The `mica` binary: the command line in `mica_cli`.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Textured meshes (textures on all surfaces) are kept in the settings folder's cache.
    mica_core::texture::displace::set_disk_cache(Some(
        mica_core::user_data::config_dir()
            .join("cache")
            .join("textures"),
    ));
    mica_cli::main(&args)
}
