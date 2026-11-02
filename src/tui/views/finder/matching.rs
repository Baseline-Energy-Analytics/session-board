//! Finder's search: nucleo fuzzy matching over each session's title, project name, project path
//! and folder, with the matched characters of each field for highlighting, and the ranking.

use crate::facts::{Session, State};
use crate::tui::widgets::{project_name, short_path};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};

/// The fields a session is searched by, as they are shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    pub title: String,
    /// The project's display name: `api`, `~ (home)`, `scratch`.
    pub name: String,
    /// The project's folder, `~`-shortened.
    pub path: String,
    /// The session's folder, `~`-shortened.
    pub cwd: String,
}

impl Fields {
    pub fn of(s: &Session, home: &str, scratch: &str) -> Fields {
        Fields {
            title: s.title.clone(),
            name: project_name(&s.project, home, scratch),
            path: short_path(&s.project, home),
            cwd: short_path(&s.cwd, home),
        }
    }

    fn all(&self) -> [&str; 4] {
        [&self.title, &self.name, &self.path, &self.cwd]
    }
}

/// Which field a highlight belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title = 0,
    Name = 1,
    Path = 2,
    Cwd = 3,
}

/// One result: a session (by its place in the snapshot, valid for one draw or key), its score
/// and the matched characters of each field (char indices, sorted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub index: usize,
    pub score: u32,
    pub fields: Fields,
    hl: [Vec<usize>; 4],
}

impl Hit {
    pub fn hl(&self, f: Field) -> &[usize] {
        &self.hl[f as usize]
    }
}

/// Between fields in the combined haystack.
const GAP: &str = "  ";

/// The matcher and its scratch space, kept between searches.
#[derive(Default)]
pub struct Search {
    matcher: Matcher,
    chars: Vec<char>,
}

impl Search {
    /// The sessions `query` finds, ranked: by state (needs you first, ended last), then by how
    /// well they match, then most recent first. Ended sessions are included whenever a query is
    /// typed, and otherwise only with `show_ended`.
    pub fn hits(&mut self, query: &str, sessions: &[Session], show_ended: bool, home: &str, scratch: &str) -> Vec<Hit> {
        let searching = !query.trim().is_empty();
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut hits = Vec::new();
        for (index, s) in sessions.iter().enumerate() {
            if s.state == State::Ended && !show_ended && !searching {
                continue;
            }
            let fields = Fields::of(s, home, scratch);
            if !searching {
                hits.push(Hit { index, score: 0, fields, hl: Default::default() });
                continue;
            }
            let hay = fields.all().join(GAP);
            let mut combined = Vec::new();
            let Some(score) = pattern.indices(Utf32Str::new(&hay, &mut self.chars), &mut self.matcher, &mut combined)
            else {
                continue;
            };
            let hl = self.highlights(&pattern, &fields, &combined);
            hits.push(Hit { index, score, fields, hl });
        }
        hits.sort_by(|a, b| {
            let (sa, sb) = (&sessions[a.index], &sessions[b.index]);
            sa.state.cmp(&sb.state).then(b.score.cmp(&a.score)).then(sb.last.total_cmp(&sa.last))
        });
        hits
    }

    /// Each field lights up where any word of the query matches it on its own (so "infra"
    /// lights the project name, its path and the folder alike). A query that only matches
    /// across fields falls back to where it matched in the combined haystack.
    fn highlights(&mut self, pattern: &Pattern, fields: &Fields, combined: &[u32]) -> [Vec<usize>; 4] {
        let mut hl: [Vec<usize>; 4] = Default::default();
        for (k, text) in fields.all().into_iter().enumerate() {
            for atom in pattern.atoms.iter().filter(|a| !a.negative) {
                let mut ind = Vec::new();
                if atom.indices(Utf32Str::new(text, &mut self.chars), &mut self.matcher, &mut ind).is_some() {
                    hl[k].extend(ind.into_iter().map(|i| i as usize));
                }
            }
        }
        if hl.iter().all(Vec::is_empty) {
            let gap = GAP.chars().count();
            let mut start = 0;
            for (k, text) in fields.all().into_iter().enumerate() {
                let len = text.chars().count();
                hl[k] = combined
                    .iter()
                    .map(|&i| i as usize)
                    .filter(|i| (start..start + len).contains(i))
                    .map(|i| i - start)
                    .collect();
                start += len + gap;
            }
        }
        for v in &mut hl {
            v.sort_unstable();
            v.dedup();
        }
        hl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str, cwd: &str, state: State, last: f64) -> Session {
        Session::new(id, title, cwd, cwd, state, last)
    }

    fn sessions() -> Vec<Session> {
        vec![
            session("a", "Fix the flaky auth test", "/home/u/work/api", State::NeedsYou, 10.0),
            session("b", "Terraform plan", "/home/u/work/infra", State::Running, 20.0),
            session("c", "Old infra notes", "/home/u/notes", State::Ended, 30.0),
            session("d", "Postgres drill", "/home/u/work/infra", State::Idle, 40.0),
        ]
    }

    fn ids(hits: &[Hit], ss: &[Session]) -> Vec<String> {
        hits.iter().map(|h| ss[h.index].session_id.clone()).collect()
    }

    #[test]
    fn fuzzy_finds_and_highlights_per_field() {
        let ss = sessions();
        let mut s = Search::default();
        let hits = s.hits("flky", &ss, false, "/home/u", "/tmp");
        assert_eq!(ids(&hits, &ss), ["a"]);
        let lit: String = hits[0].hl(Field::Title).iter().map(|&i| ss[0].title.chars().nth(i).unwrap()).collect();
        assert_eq!(lit, "flky");

        let hits = s.hits("infra", &ss, false, "/home/u", "/tmp");
        assert_eq!(ids(&hits, &ss), ["b", "d", "c"], "state order, ended last, included while searching");
        let h = &hits[0];
        assert_eq!(h.hl(Field::Name), [0, 1, 2, 3, 4]);
        assert_eq!(h.hl(Field::Cwd), [7, 8, 9, 10, 11], "~/work/infra");
        assert!(h.hl(Field::Title).is_empty());
    }

    #[test]
    fn no_query_lists_everything_but_ended_unless_asked() {
        let ss = sessions();
        let mut s = Search::default();
        assert_eq!(ids(&s.hits("", &ss, false, "/home/u", "/tmp"), &ss), ["a", "b", "d"]);
        assert_eq!(ids(&s.hits("  ", &ss, true, "/home/u", "/tmp"), &ss), ["a", "b", "d", "c"]);
        assert!(s.hits("zzzq", &ss, true, "/home/u", "/tmp").is_empty());
    }

    #[test]
    fn a_query_across_fields_falls_back_to_the_combined_match() {
        let ss = sessions();
        let mut s = Search::default();
        // "drill" is in the title and "infra" in the project: two words, each lights its own.
        let hits = s.hits("drill infra", &ss, false, "/home/u", "/tmp");
        assert_eq!(ids(&hits, &ss), ["d"]);
        assert_eq!(hits[0].hl(Field::Title), [9, 10, 11, 12, 13]);
        assert_eq!(hits[0].hl(Field::Name), [0, 1, 2, 3, 4]);
    }
}
