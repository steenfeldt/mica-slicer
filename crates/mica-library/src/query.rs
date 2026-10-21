// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Search, filters and sorting over a snapshot. Pure functions, so the view can re-run them
//! on every keystroke.

use crate::model::{Kind, Model};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Colours {
    #[default]
    Any,
    /// Has filament colour information.
    Some,
    /// More than one distinct colour.
    Multi,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sliced {
    #[default]
    Any,
    Yes,
    No,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Modified,
    /// File size on disk.
    FileSize,
    /// The largest dimension.
    Dimensions,
    PrintTime,
    Kind,
}

impl Sort {
    pub const ALL: [Sort; 6] = [
        Sort::Name,
        Sort::Modified,
        Sort::FileSize,
        Sort::Dimensions,
        Sort::PrintTime,
        Sort::Kind,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "Name",
            Sort::Modified => "Date modified",
            Sort::FileSize => "File size",
            Sort::Dimensions => "Model size",
            Sort::PrintTime => "Print time",
            Sort::Kind => "Type",
        }
    }
}

/// What the library view shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    /// Words that must all appear in the name, folder, tags or profile names.
    pub text: String,
    /// Empty: every type.
    pub kinds: BTreeSet<Kind>,
    pub printer: Option<String>,
    pub colours: Colours,
    pub sliced: Sliced,
    /// The largest dimension in mm, inclusive (models without a size never match a bound).
    pub min_size_mm: Option<f32>,
    pub max_size_mm: Option<f32>,
    /// Modified within this many days.
    pub within_days: Option<u32>,
    pub favourites: bool,
    pub tag: Option<String>,
    /// Only models with a copy under this library folder.
    pub folder: Option<PathBuf>,
}

impl Filter {
    /// Anything narrowing the list besides the search text.
    pub fn narrowed(&self) -> bool {
        let text = self.text.clone();
        *self
            != Filter {
                text,
                ..Default::default()
            }
    }
}

/// The text a model is searched by (lower case).
pub fn haystack(m: &Model) -> String {
    let mut s = String::new();
    for l in &m.locations {
        s.push_str(&l.path.to_string_lossy());
        s.push('\n');
    }
    for t in &m.user.tags {
        s.push_str(t);
        s.push('\n');
    }
    let i = &m.info;
    for p in i
        .printer
        .iter()
        .chain(i.print_profile.iter())
        .chain(i.filament_profiles.iter())
    {
        s.push_str(p);
        s.push('\n');
    }
    s.push_str(m.kind.label());
    s.to_lowercase()
}

pub fn haystacks(models: &[Model]) -> Vec<String> {
    models.iter().map(haystack).collect()
}

/// Every printer named by a model, sorted.
pub fn printers(models: &[Model]) -> Vec<String> {
    let set: BTreeSet<&str> = models
        .iter()
        .filter_map(|m| m.info.printer.as_deref())
        .collect();
    set.into_iter().map(str::to_string).collect()
}

fn largest(m: &Model) -> Option<f32> {
    m.info.size_mm.map(|s| s[0].max(s[1]).max(s[2]))
}

pub fn matches(m: &Model, hay: &str, f: &Filter, now: i64) -> bool {
    let text = f.text.to_lowercase();
    if !text.split_whitespace().all(|w| hay.contains(w)) {
        return false;
    }
    if !f.kinds.is_empty() && !f.kinds.contains(&m.kind) {
        return false;
    }
    if let Some(p) = &f.printer
        && m.info.printer.as_ref() != Some(p)
    {
        return false;
    }
    match f.colours {
        Colours::Any => {}
        Colours::Some if m.info.colours.is_empty() => return false,
        Colours::Multi if !m.info.multi_colour() => return false,
        _ => {}
    }
    match f.sliced {
        Sliced::Yes if !m.info.sliced => return false,
        Sliced::No if m.info.sliced => return false,
        _ => {}
    }
    if f.min_size_mm.is_some() || f.max_size_mm.is_some() {
        let Some(l) = largest(m) else { return false };
        if f.min_size_mm.is_some_and(|x| l < x) || f.max_size_mm.is_some_and(|x| l > x) {
            return false;
        }
    }
    if let Some(d) = f.within_days
        && m.modified() < now - i64::from(d) * 86_400
    {
        return false;
    }
    if f.favourites && !m.user.favourite {
        return false;
    }
    if let Some(t) = &f.tag
        && !m.user.tags.iter().any(|x| x.eq_ignore_ascii_case(t))
    {
        return false;
    }
    if let Some(root) = &f.folder
        && !m.locations.iter().any(|l| &l.root == root)
    {
        return false;
    }
    true
}

/// Indices of the models that match, in display order.
pub fn apply(
    models: &[Model],
    hay: &[String],
    f: &Filter,
    sort: Sort,
    descending: bool,
    now: i64,
) -> Vec<usize> {
    let mut out: Vec<usize> = (0..models.len())
        .filter(|&i| matches(&models[i], hay.get(i).map_or("", String::as_str), f, now))
        .collect();
    let name = |i: usize| models[i].name.to_lowercase();
    out.sort_by(|&a, &b| {
        let (ma, mb) = (&models[a], &models[b]);
        let o = match sort {
            Sort::Name => name(a).cmp(&name(b)),
            Sort::Modified => ma.modified().cmp(&mb.modified()),
            Sort::FileSize => ma.size().cmp(&mb.size()),
            Sort::Dimensions => largest(ma)
                .unwrap_or(-1.)
                .total_cmp(&largest(mb).unwrap_or(-1.)),
            Sort::PrintTime => ma
                .info
                .print_time_s
                .unwrap_or(-1.)
                .total_cmp(&mb.info.print_time_s.unwrap_or(-1.)),
            Sort::Kind => ma.kind.cmp(&mb.kind),
        };
        let o = if descending { o.reverse() } else { o };
        o.then_with(|| name(a).cmp(&name(b))).then(a.cmp(&b))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Info, Location, UserData};

    fn model(
        name: &str,
        kind: Kind,
        size: f32,
        printer: Option<&str>,
        colours: &[&str],
        modified: i64,
    ) -> Model {
        Model {
            id: name.into(),
            name: name.into(),
            kind,
            locations: vec![Location {
                path: PathBuf::from(format!("/lib/Toys/{name}")),
                root: "/lib".into(),
                size: (size * 10.) as u64,
                modified,
                online: true,
            }],
            info: Info {
                readable: true,
                size_mm: Some([size, 1., 1.]),
                printer: printer.map(str::to_string),
                colours: colours.iter().map(|c| c.to_string()).collect(),
                sliced: kind.is_gcode(),
                ..Default::default()
            },
            thumbnail: None,
            user: UserData::default(),
        }
    }

    #[test]
    fn search_filters_and_sort() {
        let now = 100 * 86_400;
        let mut models = vec![
            model("gear.stl", Kind::Stl, 30., None, &[], now),
            model(
                "vase.3mf",
                Kind::ThreeMf,
                150.,
                Some("XL"),
                &["#FF0000", "#00FF00"],
                now - 40 * 86_400,
            ),
            model(
                "clip.gcode",
                Kind::Gcode,
                10.,
                Some("CORE One"),
                &["#FF0000"],
                now - 86_400,
            ),
        ];
        models[0].user.tags = vec!["Workshop".into()];
        models[0].user.favourite = true;
        let hay = haystacks(&models);
        let names = |f: &Filter, s: Sort, d: bool| -> Vec<String> {
            apply(&models, &hay, f, s, d, now)
                .into_iter()
                .map(|i| models[i].name.clone())
                .collect()
        };
        let all = Filter::default();
        assert_eq!(
            names(&all, Sort::Name, false),
            ["clip.gcode", "gear.stl", "vase.3mf"]
        );
        assert_eq!(
            names(&all, Sort::Dimensions, true),
            ["vase.3mf", "gear.stl", "clip.gcode"]
        );
        let f = |g: fn(&mut Filter)| {
            let mut f = Filter::default();
            g(&mut f);
            f
        };
        assert_eq!(
            names(&f(|f| f.text = "TOYS gea".into()), Sort::Name, false),
            ["gear.stl"]
        );
        assert_eq!(
            names(&f(|f| f.text = "workshop".into()), Sort::Name, false),
            ["gear.stl"]
        );
        assert_eq!(
            names(&f(|f| f.text = "core".into()), Sort::Name, false),
            ["clip.gcode"]
        );
        assert_eq!(
            names(&f(|f| f.colours = Colours::Multi), Sort::Name, false),
            ["vase.3mf"]
        );
        assert_eq!(
            names(&f(|f| f.colours = Colours::Some), Sort::Name, false).len(),
            2
        );
        assert_eq!(
            names(&f(|f| f.sliced = Sliced::Yes), Sort::Name, false),
            ["clip.gcode"]
        );
        assert_eq!(
            names(&f(|f| f.max_size_mm = Some(50.)), Sort::Name, false).len(),
            2
        );
        assert_eq!(
            names(&f(|f| f.within_days = Some(7)), Sort::Name, false).len(),
            2
        );
        assert_eq!(
            names(&f(|f| f.favourites = true), Sort::Name, false),
            ["gear.stl"]
        );
        assert_eq!(
            names(&f(|f| f.tag = Some("workshop".into())), Sort::Name, false),
            ["gear.stl"]
        );
        assert_eq!(
            names(&f(|f| f.printer = Some("XL".into())), Sort::Name, false),
            ["vase.3mf"]
        );
        assert_eq!(
            names(
                &f(|f| {
                    f.kinds.insert(Kind::Stl);
                }),
                Sort::Name,
                false
            ),
            ["gear.stl"]
        );
        assert_eq!(printers(&models), ["CORE One", "XL"]);
        assert!(f(|f| f.favourites = true).narrowed());
        assert!(!f(|f| f.text = "x".into()).narrowed());
    }
}
