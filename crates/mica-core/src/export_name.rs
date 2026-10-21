// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! File names for exported G-code, shared by the GUI (Export, Export all plates, Send to
//! printer) and `mica slice --each-plate`, so both name a plate's file the same way: the first
//! object's name, and with several plates `_` and the plate's name (`cube20_plate_2`,
//! `benchy_gift___tom`).

/// The plate's name as a file name part: lower case, letters, numbers and `-`; anything else
/// (spaces, `/`, `.`) becomes `_`.
pub fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .to_lowercase()
}

/// The file stem of a plate's export: `first_object` is the name of the plate's first object
/// (a file name's extension is dropped), `plate` the plate's name (`Plate 2` when it has none
/// of its own) and `plates` how many plates the project has.
pub fn stem(first_object: Option<&str>, plate: &str, plates: usize) -> String {
    let first = first_object
        .map(|name| {
            std::path::Path::new(name)
                .file_stem()
                .map_or(name.to_string(), |s| s.to_string_lossy().into_owned())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "print".into());
    if plates > 1 {
        format!("{first}_{}", sanitize(plate))
    } else {
        first
    }
}

/// `stem` made unique among `taken` (compared without case, as macOS and Windows file systems
/// do): `cube`, then `cube-2`, `cube-3`… The result is added to `taken`.
pub fn unique(stem: &str, taken: &mut Vec<String>) -> String {
    let free = |s: &str, taken: &[String]| !taken.iter().any(|t| t.eq_ignore_ascii_case(s));
    let mut out = stem.to_string();
    let mut n = 2;
    while !free(&out, taken) {
        out = format!("{stem}-{n}");
        n += 1;
    }
    taken.push(out.clone());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plate_names_become_safe_file_names() {
        assert_eq!(stem(Some("cube20.stl"), "Plate 1", 1), "cube20");
        assert_eq!(stem(Some("cube20.stl"), "Plate 1", 2), "cube20_plate_1");
        assert_eq!(stem(Some("cube20"), "Gift / Tom", 3), "cube20_gift___tom");
        assert_eq!(stem(None, "Plate 2", 2), "print_plate_2");
        assert_eq!(stem(Some(""), "Plate 2", 1), "print");
    }

    #[test]
    fn repeated_names_get_a_number() {
        let mut taken = vec![];
        assert_eq!(unique("cube_a", &mut taken), "cube_a");
        assert_eq!(unique("Cube_A", &mut taken), "Cube_A-2");
        assert_eq!(unique("cube_a", &mut taken), "cube_a-3");
        assert_eq!(unique("cube_b", &mut taken), "cube_b");
    }
}
