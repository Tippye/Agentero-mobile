use super::snapshot::SyncScope;
use crate::features::paper::identity::is_paper_marker_file;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Category {
    Pdf,
    Source,
    Attachments,
    /// Handwritten ink sidecars (`marks/ink/`); mobile optional category.
    Ink,
    /// Finished translated PDFs (`translated/`); mobile optional category.
    Translated,
}

impl Category {
    pub(super) fn enabled(self, scope: &SyncScope) -> bool {
        match self {
            Self::Pdf => scope.pdf,
            Self::Source => scope.source,
            Self::Attachments => scope.attachments,
            Self::Ink => scope.ink,
            Self::Translated => scope.translated,
        }
    }

    pub(super) fn is_directory(self) -> bool {
        matches!(
            self,
            Self::Source | Self::Attachments | Self::Ink | Self::Translated
        )
    }
}

#[derive(Default)]
pub(super) struct PaperRoots {
    papers: HashSet<String>,
    collections: HashSet<String>,
}

pub(super) fn paper_roots<'a>(paths: impl IntoIterator<Item = &'a str>) -> PaperRoots {
    let papers: HashSet<String> = paths
        .into_iter()
        .filter(|path| path.starts_with("papers/"))
        .filter_map(|path| {
            let (parent, name) = path.rsplit_once('/')?;
            let main_pdf = name.rsplit_once('.').is_some_and(|(stem, ext)| {
                ext.eq_ignore_ascii_case("pdf")
                    && parent
                        .rsplit('/')
                        .next()
                        .is_some_and(|folder| stem.eq_ignore_ascii_case(folder))
            });
            (is_paper_marker_file(name) || main_pdf).then(|| parent.to_owned())
        })
        .collect();
    let collections = papers
        .iter()
        .flat_map(|path| {
            path.match_indices('/')
                .map(|(end, _)| path[..end].to_owned())
        })
        .collect();
    PaperRoots {
        papers,
        collections,
    }
}

/// Classify a vault-relative path and return the file or asset directory that
/// controls its scope. The first paper-internal directory owns its subtree.
pub(super) fn classify<'a>(path: &'a str, roots: &PaperRoots) -> Option<(Category, &'a str)> {
    let rest = path.strip_prefix("papers/")?;
    let (first_dir, _) = rest.split_once('/')?;
    let legacy = &path[.."papers/".len() + first_dir.len()];
    let root = path
        .match_indices('/')
        .map(|(pos, _)| &path[..pos])
        .find(|prefix| roots.papers.contains(*prefix))
        .or_else(|| (!roots.collections.contains(legacy)).then_some(legacy));
    if let Some(root) = root {
        let component = path[root.len() + 1..].split('/').next()?;
        let end = root.len() + 1 + component.len();
        match component {
            "source" => {
                // Layout is required for figures, citations and deferred-paper availability:
                // keep raw layout and the sidebar index always synced. The translation sidecar
                // is cache output and follows the translation-cache switch.
                let tail = &path[end..];
                if tail == "/layout.json" || tail == "/layout-index.json" {
                    return None;
                }
                if tail == "/layout-translate.json" {
                    return Some((Category::Translated, path));
                }
                return Some((Category::Source, &path[..end]));
            }
            "attachments" => return Some((Category::Attachments, &path[..end])),
            "translated" => return Some((Category::Translated, &path[..end])),
            "marks" => {
                // Only the ink subtree is an optional category; annotations
                // and other marks always sync.
                let tail = &path[end..];
                if tail == "/ink" || tail.starts_with("/ink/") {
                    return Some((Category::Ink, &path[..end + "/ink".len()]));
                }
                return None;
            }
            "assets" => return None,
            _ => {}
        }
    }
    let (parent, file) = path.rsplit_once('/')?;
    let (stem, ext) = file.rsplit_once('.')?;
    let (groups, folder) = parent.rsplit_once('/')?;
    if ext.eq_ignore_ascii_case("pdf")
        && (groups == "papers"
            || stem.eq_ignore_ascii_case(folder)
            || roots.papers.contains(parent))
    {
        return Some((Category::Pdf, path));
    }
    None
}

pub(super) fn excluded(scope: &SyncScope, path: &str, roots: &PaperRoots) -> bool {
    !scope.is_all() && classify(path, roots).is_some_and(|(category, _)| !category.enabled(scope))
}
