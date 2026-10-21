// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The object tree's model, apart from its widgets (`panels/tree.rs`): an object's parts
//! sorted into fixed kind groups (modifiers, negative volumes, support enforcers and
//! blockers, height ranges), the user's own groups inside a kind, what each row says it
//! changes, the moves a drag and drop makes, and how groups are kept in a saved project.
//!
//! A part's user group is a name on the part itself (`Volume::group`, `HeightRange::group`):
//! parts of one kind with the same name form the group, which sits in the tree where its
//! first member is. Groups are display only; nothing about slicing reads them. A saved
//! project keeps them in Mica's own metadata (`Metadata/mica.json`): a volume's as the part
//! metadata [`GROUP_KEY`], the ranges' as the object metadata [`RANGE_GROUPS_KEY`]. Neither
//! reaches the native model settings, so PrusaSlicer, OrcaSlicer and older Mica versions
//! ignore them and the G-code is unchanged.

use crate::scene::{HeightRange, Object, Sub, Volume};
use mica_core::threemf::PartKind;
use std::collections::BTreeMap;

/// Part metadata with the volume's user group (Mica's sidecar only).
pub const GROUP_KEY: &str = "mica_group";
/// Object metadata with the height ranges' user groups, one line per saved range.
pub const RANGE_GROUPS_KEY: &str = "mica_range_groups";

/// The fixed groups of the tree, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Model parts edited as volumes (an embossed text on a body).
    Parts,
    Modifiers,
    Negatives,
    Enforcers,
    Blockers,
    Ranges,
    /// Volumes of a kind Mica does not know.
    Other,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Parts,
        Kind::Modifiers,
        Kind::Negatives,
        Kind::Enforcers,
        Kind::Blockers,
        Kind::Ranges,
        Kind::Other,
    ];
    pub fn of(kind: &PartKind) -> Self {
        match kind {
            PartKind::Model => Kind::Parts,
            PartKind::Modifier => Kind::Modifiers,
            PartKind::NegativeVolume => Kind::Negatives,
            PartKind::SupportEnforcer => Kind::Enforcers,
            PartKind::SupportBlocker => Kind::Blockers,
            PartKind::Unknown(_) => Kind::Other,
        }
    }
    /// The group's title (shown spaced in capitals).
    pub fn title(self) -> &'static str {
        match self {
            Kind::Parts => "Parts",
            Kind::Modifiers => "Modifiers",
            Kind::Negatives => "Negative volumes",
            Kind::Enforcers => "Support enforcers",
            Kind::Blockers => "Support blockers",
            Kind::Ranges => "Height ranges",
            Kind::Other => "Other volumes",
        }
    }
    /// A stable name for remembered fold states.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Parts => "parts",
            Kind::Modifiers => "modifiers",
            Kind::Negatives => "negatives",
            Kind::Enforcers => "enforcers",
            Kind::Blockers => "blockers",
            Kind::Ranges => "ranges",
            Kind::Other => "other",
        }
    }
}

/// The kind of a part of `o`.
pub fn kind_of(o: &Object, sub: Sub) -> Option<Kind> {
    match sub {
        Sub::Volume(id) => o.volume(id).map(|v| Kind::of(&v.kind)),
        Sub::Range(id) => o.range(id).map(|_| Kind::Ranges),
    }
}

/// The user group of a part ("" for none).
pub fn group_of(o: &Object, sub: Sub) -> Option<&str> {
    match sub {
        Sub::Volume(id) => o.volume(id).map(|v| v.group.as_str()),
        Sub::Range(id) => o.range(id).map(|r| r.group.as_str()),
    }
}

/// One entry inside a kind group: a part, or a user group with its parts.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Part(Sub),
    Group { name: String, members: Vec<Sub> },
}

/// A kind group of an object: what the tree lists under the object.
#[derive(Clone, Debug, PartialEq)]
pub struct KindGroup {
    pub kind: Kind,
    pub nodes: Vec<Node>,
    /// Parts in the kind.
    pub count: usize,
}

/// The parts of a kind in tree order (the scene's order).
fn parts_of(o: &Object, kind: Kind) -> Vec<(Sub, &str)> {
    if kind == Kind::Ranges {
        return o
            .height_ranges
            .iter()
            .map(|r| (Sub::Range(r.id), r.group.as_str()))
            .collect();
    }
    o.volumes
        .iter()
        .filter(|v| Kind::of(&v.kind) == kind)
        .map(|v| (Sub::Volume(v.id), v.group.as_str()))
        .collect()
}

/// The object's non-empty kind groups in display order, user groups gathered where their
/// first member is.
pub fn groups(o: &Object) -> Vec<KindGroup> {
    let mut out = Vec::new();
    for kind in Kind::ALL {
        let parts = parts_of(o, kind);
        if parts.is_empty() {
            continue;
        }
        let mut nodes: Vec<Node> = Vec::new();
        for (sub, group) in &parts {
            if group.is_empty() {
                nodes.push(Node::Part(*sub));
                continue;
            }
            match nodes
                .iter_mut()
                .find(|n| matches!(n, Node::Group { name, .. } if name == group))
            {
                Some(Node::Group { members, .. }) => members.push(*sub),
                _ => nodes.push(Node::Group {
                    name: group.to_string(),
                    members: vec![*sub],
                }),
            }
        }
        out.push(KindGroup {
            kind,
            count: parts.len(),
            nodes,
        });
    }
    out
}

/// Every part of the object in the order the tree lists them (Tab cycles through these).
pub fn tree_order(o: &Object) -> Vec<Sub> {
    let mut out = Vec::new();
    for g in groups(o) {
        for n in g.nodes {
            match n {
                Node::Part(s) => out.push(s),
                Node::Group { members, .. } => out.extend(members),
            }
        }
    }
    out
}

/// The next part after `current` in tree order (`back`: the one before); past either end it
/// comes back to the object itself (`None`).
pub fn cycle(o: &Object, current: Option<Sub>, back: bool) -> Option<Sub> {
    let order = tree_order(o);
    if order.is_empty() {
        return None;
    }
    let at = current.and_then(|c| order.iter().position(|s| *s == c));
    match (at, back) {
        (None, false) => order.first().copied(),
        (None, true) => order.last().copied(),
        (Some(i), false) => order.get(i + 1).copied(),
        (Some(0), true) => None,
        (Some(i), true) => order.get(i - 1).copied(),
    }
}

/// A group name for display: one line, trimmed.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `Group 1`, `Group 2`, … not yet used in the kind.
pub fn new_group_name(o: &Object, kind: Kind) -> String {
    let taken: Vec<&str> = parts_of(o, kind).into_iter().map(|(_, g)| g).collect();
    (1..)
        .map(|n| format!("Group {n}"))
        .find(|n| !taken.contains(&n.as_str()))
        .unwrap_or_else(|| "Group".into())
}

fn set_group(o: &mut Object, sub: Sub, name: &str) {
    match sub {
        Sub::Volume(id) => {
            if let Some(v) = o.volume_mut(id) {
                v.group = name.to_string();
            }
        }
        Sub::Range(id) => {
            if let Some(r) = o.range_mut(id) {
                r.group = name.to_string();
            }
        }
    }
}

/// Puts `parts` (of one kind) into a group `name`; returns false when nothing changed.
pub fn group(o: &mut Object, parts: &[Sub], name: &str) -> bool {
    let name = clean_name(name);
    if name.is_empty() || parts.is_empty() {
        return false;
    }
    let before = o.clone();
    for &s in parts {
        set_group(o, s, &name);
    }
    *o != before
}

/// Dissolves the group `name` of `kind`: its parts stay where they are, ungrouped.
pub fn ungroup(o: &mut Object, kind: Kind, name: &str) -> bool {
    rename_group(o, kind, name, "")
}

/// Renames a group (an empty name ungroups).
pub fn rename_group(o: &mut Object, kind: Kind, name: &str, new: &str) -> bool {
    let new = clean_name(new);
    let members: Vec<Sub> = parts_of(o, kind)
        .into_iter()
        .filter(|(_, g)| *g == name)
        .map(|(s, _)| s)
        .collect();
    if members.is_empty() || new == name {
        return false;
    }
    for s in members {
        set_group(o, s, &new);
    }
    true
}

/// Where a dragged part is dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    /// Before (`after: false`) or after another part of the same kind, joining its group.
    Beside { target: Sub, after: bool },
    /// Into a user group of the kind (at its end).
    IntoGroup(String),
    /// On the kind's header: out of any group, to the end of the kind.
    OutOfGroups,
}

/// Moves part `part` as dropped. Parts only move within their own object and kind; height
/// ranges keep their height order (only their group changes). Returns false when the drop
/// does not apply or changes nothing.
pub fn move_part(o: &mut Object, part: Sub, drop: &Drop) -> bool {
    let Some(kind) = kind_of(o, part) else {
        return false;
    };
    let before = o.clone();
    match drop {
        Drop::Beside { target, after } => {
            if *target == part || kind_of(o, *target) != Some(kind) {
                return false;
            }
            let group = group_of(o, *target).unwrap_or("").to_string();
            set_group(o, part, &group);
            if let (Sub::Volume(id), Sub::Volume(t)) = (part, *target) {
                let from = o.volumes.iter().position(|v| v.id == id);
                if let Some(from) = from {
                    let v = o.volumes.remove(from);
                    let at = o.volumes.iter().position(|v| v.id == t).unwrap_or(0);
                    o.volumes.insert(if *after { at + 1 } else { at }, v);
                }
            }
        }
        Drop::IntoGroup(name) => {
            let members: Vec<Sub> = parts_of(o, kind)
                .into_iter()
                .filter(|(s, g)| g == name && *s != part)
                .map(|(s, _)| s)
                .collect();
            let Some(&last) = members.last() else {
                return false;
            };
            return move_part(
                o,
                part,
                &Drop::Beside {
                    target: last,
                    after: true,
                },
            );
        }
        Drop::OutOfGroups => {
            set_group(o, part, "");
            if let Sub::Volume(id) = part
                && let Some(from) = o.volumes.iter().position(|v| v.id == id)
            {
                let v = o.volumes.remove(from);
                let at = o
                    .volumes
                    .iter()
                    .rposition(|w| Kind::of(&w.kind) == kind)
                    .map_or(o.volumes.len(), |i| i + 1);
                o.volumes.insert(at, v);
            }
        }
    }
    *o != before
}

/// What one override reads like in a row: `infill 40 %`, `gyroid`, `4 walls`.
pub fn short_override(key: &str, raw: &str) -> String {
    let v = raw.trim();
    let pretty = || crate::panels::sidebar::pretty_value(key, v);
    match key {
        "fill_density" => format!("infill {}", pretty()),
        "fill_pattern" | "top_fill_pattern" | "bottom_fill_pattern" => pretty().to_lowercase(),
        "perimeters" => format!("{v} walls"),
        "layer_height" => format!("{v} mm layers"),
        "top_solid_layers" => format!("{v} top"),
        "bottom_solid_layers" => format!("{v} bottom"),
        "ironing" => if v == "1" { "ironing" } else { "no ironing" }.into(),
        "support_material" => if v == "1" { "supports" } else { "no supports" }.into(),
        "seam_position" => format!("seam {}", pretty().to_lowercase()),
        _ => format!(
            "{} {}",
            crate::settings::label(key).to_lowercase(),
            pretty()
        ),
    }
}

/// The overrides as a row summary, the most telling first: `infill 40 % · gyroid`.
pub fn summary(overrides: &BTreeMap<String, String>) -> Vec<String> {
    const FIRST: [&str; 6] = [
        "layer_height",
        "fill_density",
        "fill_pattern",
        "perimeters",
        "ironing",
        "support_material",
    ];
    let mut keys: Vec<&String> = overrides.keys().collect();
    keys.sort_by_key(|k| FIRST.iter().position(|f| f == k).unwrap_or(FIRST.len()));
    keys.into_iter()
        .map(|k| short_override(k, &overrides[k]))
        .collect()
}

/// What a part row says: its overrides, or its kind.
pub fn part_text(o: &Object, sub: Sub) -> (String, bool) {
    let (overrides, kind) = match sub {
        Sub::Volume(id) => match o.volume(id) {
            Some(v) => (&v.overrides, v.kind_label()),
            None => return (String::new(), false),
        },
        Sub::Range(id) => match o.range(id) {
            Some(r) => (&r.overrides, "Height range"),
            None => return (String::new(), false),
        },
    };
    let s = summary(overrides);
    if s.is_empty() {
        (kind.to_string(), false)
    } else {
        (s.into_iter().take(2).collect::<Vec<_>>().join(" · "), true)
    }
}

/// A part's name as rows show it.
pub fn part_name(o: &Object, sub: Sub) -> String {
    match sub {
        Sub::Volume(id) => o.volume(id).map(|v| v.name.clone()).unwrap_or_default(),
        Sub::Range(id) => o
            .range(id)
            .map(|r| format!("Range {}", r.label()))
            .unwrap_or_default(),
    }
}

/// The volume's group as part metadata for a saved project.
pub fn volume_metadata(v: &Volume) -> Option<(String, String)> {
    (!v.group.is_empty()).then(|| (GROUP_KEY.to_string(), v.group.clone()))
}

/// The ranges' groups as object metadata, one line per range in `ranges` (saved order);
/// `None` when no range has a group.
pub fn range_metadata(ranges: &[&HeightRange]) -> Option<(String, String)> {
    ranges.iter().any(|r| !r.group.is_empty()).then(|| {
        (
            RANGE_GROUPS_KEY.to_string(),
            ranges
                .iter()
                .map(|r| r.group.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })
}

/// Groups back from an imported object's source: the non-model parts' [`GROUP_KEY`] onto
/// the editable volumes (same order), [`RANGE_GROUPS_KEY`] onto the ranges.
pub fn restore(
    source: &mica_core::threemf::Object,
    volumes: &mut [Volume],
    ranges: &mut [HeightRange],
) {
    let groups: Vec<String> = source
        .parts
        .iter()
        .filter(|p| !matches!(p.kind, PartKind::Model | PartKind::Unknown(_)))
        .map(|p| {
            p.metadata
                .get(GROUP_KEY)
                .map(|g| clean_name(g))
                .unwrap_or_default()
        })
        .collect();
    let mut editable = volumes.iter_mut().filter(|v| v.kind != PartKind::Model);
    for g in groups {
        match editable.next() {
            Some(v) => v.group = g,
            None => break,
        }
    }
    if let Some(text) = source.metadata.get(RANGE_GROUPS_KEY) {
        for (r, g) in ranges.iter_mut().zip(text.split('\n')) {
            r.group = clean_name(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{config::Config, input::InputObject, print_job::ToolAssignments};

    pub(crate) fn object() -> Object {
        let mesh = mica_core::mesh_edit::cuboid([10., 10., 10.]);
        let mut o = Object::new(InputObject::plain(
            "cube",
            0,
            0,
            mesh.clone(),
            ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ));
        for (name, kind) in [
            ("A", PartKind::Modifier),
            ("hole", PartKind::NegativeVolume),
            ("B", PartKind::Modifier),
            ("C", PartKind::Modifier),
            ("sup", PartKind::SupportEnforcer),
        ] {
            o.volumes.push(Volume::new(name, kind, mesh.clone()));
        }
        o.height_ranges.push(HeightRange::new(0., 2.));
        o.height_ranges.push(HeightRange::new(2., 4.));
        o
    }

    fn id(o: &Object, name: &str) -> Sub {
        Sub::Volume(o.volumes.iter().find(|v| v.name == name).unwrap().id)
    }

    #[test]
    fn parts_sort_into_kind_groups_in_a_fixed_order() {
        let o = object();
        let g = groups(&o);
        let kinds: Vec<Kind> = g.iter().map(|g| g.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Modifiers,
                Kind::Negatives,
                Kind::Enforcers,
                Kind::Ranges
            ]
        );
        assert_eq!(g[0].count, 3);
        assert_eq!(
            g[0].nodes,
            vec![
                Node::Part(id(&o, "A")),
                Node::Part(id(&o, "B")),
                Node::Part(id(&o, "C"))
            ]
        );
        // Tree order runs kind by kind, and Tab comes back to the object past the end.
        let order = tree_order(&o);
        assert_eq!(order.len(), 7);
        assert_eq!(order[3], id(&o, "hole"));
        assert_eq!(cycle(&o, None, false), Some(order[0]));
        assert_eq!(cycle(&o, Some(order[0]), false), Some(order[1]));
        assert_eq!(cycle(&o, Some(order[6]), false), None);
        assert_eq!(cycle(&o, None, true), Some(order[6]));
        assert_eq!(cycle(&o, Some(order[0]), true), None);
    }

    #[test]
    fn user_groups_gather_rename_ungroup_and_move() {
        let mut o = object();
        let (a, b, c) = (id(&o, "A"), id(&o, "B"), id(&o, "C"));
        assert_eq!(new_group_name(&o, Kind::Modifiers), "Group 1");
        assert!(group(&mut o, &[b, c], "Hull\ndetails "));
        let g = &groups(&o)[0];
        assert_eq!(
            g.nodes,
            vec![
                Node::Part(a),
                Node::Group {
                    name: "Hull details".into(),
                    members: vec![b, c]
                }
            ]
        );
        assert_eq!(new_group_name(&o, Kind::Modifiers), "Group 1");
        // Drag A into the group: it joins at the end, and the volume order follows.
        assert!(move_part(
            &mut o,
            a,
            &Drop::IntoGroup("Hull details".into())
        ));
        let names: Vec<&str> = o.volumes.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["hole", "B", "C", "A", "sup"]);
        // Before B: reorders inside the group.
        assert!(move_part(
            &mut o,
            a,
            &Drop::Beside {
                target: b,
                after: false
            }
        ));
        let names: Vec<&str> = o.volumes.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["hole", "A", "B", "C", "sup"]);
        // Out of the group, to the end of the kind.
        assert!(move_part(&mut o, a, &Drop::OutOfGroups));
        assert_eq!(group_of(&o, a), Some(""));
        let names: Vec<&str> = o.volumes.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["hole", "B", "C", "A", "sup"]);
        // A drop onto another kind does nothing.
        let hole = id(&o, "hole");
        assert!(!move_part(
            &mut o,
            a,
            &Drop::Beside {
                target: hole,
                after: true
            }
        ));
        assert!(rename_group(
            &mut o,
            Kind::Modifiers,
            "Hull details",
            "Keel"
        ));
        assert_eq!(group_of(&o, b), Some("Keel"));
        assert!(ungroup(&mut o, Kind::Modifiers, "Keel"));
        assert!(
            groups(&o)[0]
                .nodes
                .iter()
                .all(|n| matches!(n, Node::Part(_)))
        );
        // Ranges group too, and keep their height order.
        let r0 = Sub::Range(o.height_ranges[0].id);
        assert!(group(&mut o, &[r0], "Top"));
        assert!(matches!(groups(&o)[3].nodes[0], Node::Group { .. }));
    }

    #[test]
    fn summaries_say_what_a_part_changes() {
        let mut o = object();
        let a = id(&o, "A");
        assert_eq!(part_text(&o, a), ("Modifier".to_string(), false));
        let v = o.volume_mut(match a {
            Sub::Volume(i) => i,
            _ => unreachable!(),
        });
        let v = v.unwrap();
        v.overrides.insert("fill_pattern".into(), "gyroid".into());
        v.overrides.insert("fill_density".into(), "40%".into());
        v.overrides.insert("perimeters".into(), "4".into());
        assert_eq!(part_text(&o, a), ("infill 40 % · gyroid".to_string(), true));
        assert_eq!(short_override("layer_height", "0.12"), "0.12 mm layers");
    }
}
