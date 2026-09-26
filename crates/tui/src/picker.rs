//! The filter picker: a small form that composes a filter line without the
//! human having to know the grammar. Each field cycles through values
//! (←/→), ↑/↓ moves between fields, Enter applies, Esc cancels, `e` drops
//! to the raw line for anyone who prefers typing. The composed text is the
//! same string `filter::parse` accepts, so the two paths cannot diverge.

use swamp_core::report::Report;

pub const SIZES: &[&str] = &[
    "off", "1MB", "10MB", "50MB", "100MB", "500MB", "1GB", "5GB", "20GB",
];
/// The ladder a growth window is chosen from. Only rungs the store can
/// actually answer are offered (see [`windows_for`]).
pub const WINDOW_LADDER: &[(&str, u64)] = &[
    ("1h", 3_600),
    ("6h", 21_600),
    ("24h", 86_400),
    ("7d", 604_800),
    ("30d", 2_592_000),
    ("90d", 7_776_000),
    ("1y", 31_536_000),
];

/// Windows the store can honor given `history_secs` of observations.
/// Offering `1y` against four hours of history would report a year of
/// growth the tool never observed.
pub fn windows_for(history_secs: Option<u64>) -> Vec<String> {
    let Some(span) = history_secs else {
        return vec!["all history".to_string()];
    };
    let mut out: Vec<String> = WINDOW_LADDER
        .iter()
        .filter(|(_, secs)| *secs <= span)
        .map(|(label, _)| label.to_string())
        .collect();
    if !WINDOW_LADDER.iter().any(|(_, secs)| *secs == span) {
        out.push(format!("all history ({})", human_span(span)));
    }
    out
}

/// The duration a window label means, resolving "all history".
pub fn window_secs(label: &str, history_secs: Option<u64>) -> u64 {
    if label.starts_with("all history") {
        return history_secs.unwrap_or(0);
    }
    WINDOW_LADDER
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, s)| *s)
        .unwrap_or(0)
}

/// A window label as the filter grammar spells it (`4h`, `7d`).
pub fn window_filter_text(label: &str, history_secs: Option<u64>) -> String {
    if label.starts_with("all history") {
        human_span(history_secs.unwrap_or(0))
    } else {
        label.to_string()
    }
}

pub fn human_span(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}m", (secs / 60).max(1))
    }
}
pub const IDLES: &[&str] = &["off", "24h", "48h", "7d", "30d", "90d"];
pub const KINDS: &[&str] = &["any", "build", "deps", "cache", "git", "source", "docker"];
pub const PRS: &[&str] = &["any", "open", "merged", "closed", "none"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub field: usize,
    pub growth_gt: bool,
    pub size_ix: usize,
    pub window_ix: usize,
    pub kind_ix: usize,
    /// Index into `projects`; 0 = any.
    pub project_ix: usize,
    /// Type-to-narrow buffer for the project field.
    pub project_query: String,
    pub idle_ix: usize,
    pub merge_complete: bool,
    pub pr_ix: usize,
    /// Index into `types`; 0 = any.
    pub type_ix: usize,
    /// Minimum unit size (`size > X`); index into `SIZES`, 0 = off.
    pub min_size_ix: usize,
    /// Artifact age (`age > X`); index into `IDLES`, 0 = off.
    pub age_ix: usize,
    pub projects: Vec<String>,
    /// `any` then every ecosystem tag the table knows.
    pub types: Vec<String>,
    /// Seconds of observation history the store holds; bounds `windows`.
    pub history_secs: Option<u64>,
    pub windows: Vec<String>,
}

pub const FIELDS: &[&str] = &[
    "growth",
    "window",
    "kind",
    "project",
    "idle",
    "merge-complete",
    "pr",
    "type",
    "size",
    "age",
];

impl Picker {
    /// Seeds the form from the report (project names) and, when it parses,
    /// the current filter text so opening the picker shows what is applied.
    pub fn from_report(report: &Report, current: &str, history_secs: Option<u64>) -> Self {
        let mut projects: Vec<String> = report.projects.iter().map(|p| p.name.clone()).collect();
        projects.sort();
        projects.dedup();
        let windows = windows_for(history_secs);
        let mut p = Picker {
            field: 0,
            growth_gt: true,
            size_ix: 4, // 100MB
            window_ix: windows.len().saturating_sub(1),
            kind_ix: 0,
            project_ix: 0,
            project_query: String::new(),
            idle_ix: 0,
            merge_complete: false,
            pr_ix: 0,
            type_ix: 0,
            min_size_ix: 0,
            age_ix: 0,
            projects,
            types: std::iter::once("any".to_string())
                .chain(
                    swamp_core::ecosystem::ECOSYSTEMS
                        .iter()
                        .map(|e| e.tag.to_string()),
                )
                .collect(),
            history_secs,
            windows,
        };
        p.seed_from_text(current);
        p
    }

    fn seed_from_text(&mut self, text: &str) {
        let toks: Vec<&str> = text.split_whitespace().collect();
        let mut i = 0;
        if text.trim() == "0" || text.trim().is_empty() {
            self.size_ix = 0;
            return;
        }
        let mut saw_growth = false;
        while i < toks.len() {
            match toks[i] {
                "growth" if i + 4 < toks.len() + 1 && i + 2 < toks.len() => {
                    saw_growth = true;
                    self.growth_gt = toks[i + 1] == ">";
                    if let Some(ix) = SIZES
                        .iter()
                        .position(|s| s.eq_ignore_ascii_case(toks[i + 2]))
                    {
                        self.size_ix = ix;
                    }
                    if i + 4 < toks.len()
                        && let Some(ix) = self
                            .windows
                            .iter()
                            .position(|w| w.eq_ignore_ascii_case(toks[i + 4]))
                    {
                        self.window_ix = ix;
                    }
                    i += 5;
                }
                "idle" if i + 2 < toks.len() => {
                    if let Some(ix) = IDLES
                        .iter()
                        .position(|d| d.eq_ignore_ascii_case(toks[i + 2]))
                    {
                        self.idle_ix = ix;
                    }
                    i += 3;
                }
                "age" if i + 2 < toks.len() => {
                    if let Some(ix) = IDLES
                        .iter()
                        .position(|d| d.eq_ignore_ascii_case(toks[i + 2]))
                    {
                        self.age_ix = ix;
                    }
                    i += 3;
                }
                "size" if i + 2 < toks.len() => {
                    if let Some(ix) = SIZES
                        .iter()
                        .position(|d| d.eq_ignore_ascii_case(toks[i + 2]))
                    {
                        self.min_size_ix = ix;
                    }
                    i += 3;
                }
                t if t.starts_with("type:") => {
                    self.type_ix = self
                        .types
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(&t[5..]))
                        .unwrap_or(0);
                    i += 1;
                }
                "merge-complete" => {
                    self.merge_complete = true;
                    i += 1;
                }
                t if t.starts_with("kind:") => {
                    let k = &t[5..];
                    self.kind_ix = KINDS
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(k))
                        .unwrap_or_else(|| match k.to_ascii_lowercase().as_str() {
                            "buildoutput" => 1,
                            "dependencytree" => 2,
                            _ => 0,
                        });
                    i += 1;
                }
                t if t.starts_with("project:") => {
                    let n = &t[8..];
                    self.project_ix = self
                        .projects
                        .iter()
                        .position(|p| p.eq_ignore_ascii_case(n))
                        .map(|ix| ix + 1)
                        .unwrap_or(0);
                    i += 1;
                }
                t if t.starts_with("pr:") => {
                    self.pr_ix = PRS
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(&t[3..]))
                        .unwrap_or(0);
                    i += 1;
                }
                _ => i += 1,
            }
        }
        if !saw_growth {
            self.size_ix = 0;
        }
    }

    pub fn up(&mut self) {
        self.field = self.field.saturating_sub(1);
    }
    pub fn down(&mut self) {
        self.field = (self.field + 1).min(FIELDS.len() - 1);
    }

    /// The project choices narrowed by the type-to-narrow buffer; index 0
    /// is always "any".
    pub fn project_choices(&self) -> Vec<String> {
        let q = self.project_query.to_ascii_lowercase();
        let mut v = vec!["any".to_string()];
        v.extend(
            self.projects
                .iter()
                .filter(|p| q.is_empty() || p.to_ascii_lowercase().contains(&q))
                .cloned(),
        );
        v
    }

    pub fn cycle(&mut self, delta: i32) {
        fn step(ix: usize, len: usize, delta: i32) -> usize {
            ((ix as i32 + delta).rem_euclid(len as i32)) as usize
        }
        match self.field {
            0 => self.size_ix = step(self.size_ix, SIZES.len(), delta),
            1 => self.window_ix = step(self.window_ix, self.windows.len(), delta),
            2 => self.kind_ix = step(self.kind_ix, KINDS.len(), delta),
            3 => {
                let n = self.project_choices().len();
                self.project_ix = step(self.project_ix, n, delta);
            }
            4 => self.idle_ix = step(self.idle_ix, IDLES.len(), delta),
            5 => self.merge_complete = !self.merge_complete,
            6 => self.pr_ix = step(self.pr_ix, PRS.len(), delta),
            7 => self.type_ix = step(self.type_ix, self.types.len(), delta),
            8 => self.min_size_ix = step(self.min_size_ix, SIZES.len(), delta),
            9 => self.age_ix = step(self.age_ix, IDLES.len(), delta),
            _ => {}
        }
    }

    /// Space on the growth field flips grew (>) / shrank (<).
    pub fn flip_op(&mut self) {
        if self.field == 0 {
            self.growth_gt = !self.growth_gt;
        }
    }

    pub fn type_char(&mut self, c: char) {
        if self.field == 3 {
            self.project_query.push(c);
            self.project_ix = if self.project_choices().len() > 1 {
                1
            } else {
                0
            };
        }
    }
    pub fn backspace(&mut self) {
        if self.field == 3 {
            self.project_query.pop();
            self.project_ix = 0;
        }
    }

    /// The filter line this form composes. `0` when everything is off.
    pub fn compose(&self) -> String {
        let mut parts = Vec::new();
        // With no observations there is nothing to compare against, so a
        // growth predicate cannot be honored and is not composed.
        if self.size_ix > 0 && self.history_secs.is_some() {
            parts.push(format!(
                "growth {} {} in {}",
                if self.growth_gt { ">" } else { "<" },
                SIZES[self.size_ix],
                window_filter_text(&self.windows[self.window_ix], self.history_secs)
            ));
        }
        if self.kind_ix > 0 {
            let k = match KINDS[self.kind_ix] {
                "build" => "BuildOutput",
                "deps" => "DependencyTree",
                "cache" => "Cache",
                "git" => "Git",
                "source" => "Source",
                "docker" => "DockerImage",
                other => other,
            };
            parts.push(format!("kind:{k}"));
        }
        let choices = self.project_choices();
        if self.project_ix > 0 && self.project_ix < choices.len() {
            parts.push(format!("project:{}", choices[self.project_ix]));
        }
        if self.idle_ix > 0 {
            parts.push(format!("idle > {}", IDLES[self.idle_ix]));
        }
        if self.merge_complete {
            parts.push("merge-complete".into());
        }
        if self.pr_ix > 0 {
            parts.push(format!("pr:{}", PRS[self.pr_ix]));
        }
        if self.type_ix > 0 && self.type_ix < self.types.len() {
            parts.push(format!("type:{}", self.types[self.type_ix]));
        }
        if self.min_size_ix > 0 {
            parts.push(format!("size > {}", SIZES[self.min_size_ix]));
        }
        if self.age_ix > 0 {
            parts.push(format!("age > {}", IDLES[self.age_ix]));
        }
        if parts.is_empty() {
            "0".into()
        } else {
            parts.join(" ")
        }
    }

    /// Rendered field lines for the form.
    pub fn lines(&self) -> Vec<(String, String, bool)> {
        let growth = if self.history_secs.is_none() {
            "off — no observations yet".to_string()
        } else if self.size_ix == 0 {
            "off".to_string()
        } else {
            format!(
                "{} {}   (Space: grew/shrank)",
                if self.growth_gt {
                    "grew more than"
                } else {
                    "shrank more than"
                },
                SIZES[self.size_ix]
            )
        };
        let window = match self.history_secs {
            Some(span) => format!(
                "last {}   (history: {})",
                self.windows[self.window_ix],
                human_span(span)
            ),
            None => "last —   (no observations yet)".to_string(),
        };
        let choices = self.project_choices();
        let project = if self.project_query.is_empty() {
            choices
                .get(self.project_ix)
                .cloned()
                .unwrap_or_else(|| "any".into())
        } else {
            format!(
                "{}   (typing: {}▏ {} match{})",
                choices
                    .get(self.project_ix)
                    .cloned()
                    .unwrap_or_else(|| "any".into()),
                self.project_query,
                choices.len().saturating_sub(1),
                if choices.len() == 2 { "" } else { "es" }
            )
        };
        vec![
            ("growth".into(), growth, self.field == 0),
            ("window".into(), window, self.field == 1),
            ("kind".into(), KINDS[self.kind_ix].into(), self.field == 2),
            ("project".into(), project, self.field == 3),
            ("idle".into(), IDLES[self.idle_ix].into(), self.field == 4),
            (
                "merge-complete".into(),
                if self.merge_complete { "on" } else { "off" }.into(),
                self.field == 5,
            ),
            ("pr".into(), PRS[self.pr_ix].into(), self.field == 6),
            (
                "type".into(),
                {
                    let t = self.types.get(self.type_ix).cloned().unwrap_or_default();
                    match swamp_core::ecosystem::name_for(&t) {
                        Some(n) => {
                            format!("{} {t} ({n})", swamp_core::ecosystem::glyph_for(&t))
                        }
                        None => t,
                    }
                },
                self.field == 7,
            ),
            (
                "size".into(),
                if self.min_size_ix == 0 {
                    "any".to_string()
                } else {
                    format!("larger than {}", SIZES[self.min_size_ix])
                },
                self.field == 8,
            ),
            (
                "age".into(),
                if self.age_ix == 0 {
                    "any".to_string()
                } else {
                    format!("not written for {}", IDLES[self.age_ix])
                },
                self.field == 9,
            ),
        ]
    }
}

/// Tab completion for the raw filter line: completes the last token
/// against keywords, kinds, project names, sizes and durations.
pub fn complete(text: &str, projects: &[String]) -> Vec<String> {
    let trimmed = text.trim_end();
    let ends_space = text.ends_with(' ') || text.is_empty();
    let last = if ends_space {
        ""
    } else {
        trimmed.rsplit(' ').next().unwrap_or("")
    };
    let mut cands: Vec<String> = Vec::new();
    if let Some(t) = last.strip_prefix("type:") {
        cands.extend(
            swamp_core::ecosystem::ECOSYSTEMS
                .iter()
                .map(|e| e.tag)
                .filter(|x| x.starts_with(t.to_ascii_lowercase().as_str()))
                .map(|x| format!("type:{x}")),
        );
    } else if let Some(k) = last.strip_prefix("kind:") {
        cands.extend(
            [
                "BuildOutput",
                "DependencyTree",
                "Cache",
                "Git",
                "Source",
                "DockerImage",
                "DockerBuildCache",
                "DockerVolume",
            ]
            .iter()
            .filter(|x| x.to_ascii_lowercase().starts_with(&k.to_ascii_lowercase()))
            .map(|x| format!("kind:{x}")),
        );
    } else if let Some(p) = last.strip_prefix("project:") {
        cands.extend(
            projects
                .iter()
                .filter(|x| x.to_ascii_lowercase().starts_with(&p.to_ascii_lowercase()))
                .map(|x| format!("project:{x}")),
        );
    } else if let Some(p) = last.strip_prefix("pr:") {
        cands.extend(
            PRS[1..]
                .iter()
                .filter(|x| x.starts_with(p))
                .map(|x| format!("pr:{x}")),
        );
    } else {
        // Context is the tokens *before* the partial one being completed.
        let mut prev: Vec<&str> = trimmed.split_whitespace().collect();
        if !ends_space {
            prev.pop();
        }
        let after = |n: usize| prev.len() >= n;
        // contextual: after "growth >" expect a size; after "in" or "idle >" a duration
        if after(2) && prev[prev.len() - 1] == "in"
            || (after(2)
                && (prev[prev.len() - 2] == "idle" || prev[prev.len() - 2] == "age")
                && prev[prev.len() - 1] == ">")
        {
            cands.extend(
                WINDOW_LADDER
                    .iter()
                    .map(|(l, _)| *l)
                    .chain(IDLES[1..].iter().copied())
                    .filter(|x| x.starts_with(last))
                    .map(|x| x.to_string()),
            );
        } else if after(2)
            && (prev[prev.len() - 2] == "growth" || prev[prev.len() - 2] == "size")
            && (prev[prev.len() - 1] == ">" || prev[prev.len() - 1] == "<")
        {
            cands.extend(
                SIZES[1..]
                    .iter()
                    .filter(|x| {
                        x.to_ascii_lowercase()
                            .starts_with(&last.to_ascii_lowercase())
                    })
                    .map(|x| x.to_string()),
            );
        } else {
            cands.extend(
                [
                    "growth > ",
                    "kind:",
                    "project:",
                    "idle > ",
                    "merge-complete",
                    "pr:",
                    "type:",
                    "size > ",
                    "age > ",
                ]
                .iter()
                .filter(|x| x.starts_with(last))
                .map(|x| x.to_string()),
            );
        }
    }
    cands.sort();
    cands.dedup();
    cands
}

/// Applies the single completion (or the shared prefix of several) to the
/// text's last token. Returns the new text and the remaining candidates.
pub fn apply_completion(text: &str, projects: &[String]) -> (String, Vec<String>) {
    let cands = complete(text, projects);
    if cands.is_empty() {
        return (text.to_string(), cands);
    }
    let ends_space = text.ends_with(' ') || text.is_empty();
    let head = if ends_space {
        text.to_string()
    } else {
        match text.rfind(' ') {
            Some(i) => text[..=i].to_string(),
            None => String::new(),
        }
    };
    let fill = if cands.len() == 1 {
        cands[0].clone()
    } else {
        // longest common prefix
        let mut pre = cands[0].clone();
        for c in &cands[1..] {
            while !c.starts_with(&pre) {
                pre.pop();
            }
        }
        pre
    };
    (format!("{head}{fill}"), cands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_round_trips_through_the_core_parser() {
        let mut p = Picker {
            field: 0,
            growth_gt: true,
            size_ix: 4,
            window_ix: 1,
            kind_ix: 1,
            project_ix: 1,
            project_query: String::new(),
            idle_ix: 2,
            merge_complete: true,
            pr_ix: 1,
            type_ix: 0,
            min_size_ix: 0,
            age_ix: 0,
            projects: vec!["mole".into(), "roon-knob".into()],
            types: vec!["any".into(), "rs".into()],
            history_secs: Some(30 * 86_400),
            windows: windows_for(Some(30 * 86_400)),
        };
        p.window_ix = p
            .windows
            .iter()
            .position(|w| w == "7d")
            .expect("7d offered");
        let text = p.compose();
        assert_eq!(
            text,
            "growth > 100MB in 7d kind:BuildOutput project:mole idle > 48h merge-complete pr:open"
        );
        assert!(swamp_core::filter::parse(&text).is_ok());
        p.size_ix = 0;
        p.kind_ix = 0;
        p.project_ix = 0;
        p.idle_ix = 0;
        p.merge_complete = false;
        p.pr_ix = 0;
        assert_eq!(p.compose(), "0");
        p.type_ix = 1;
        p.min_size_ix = 5;
        p.age_ix = 3;
        assert_eq!(p.compose(), "type:rs size > 500MB age > 7d");
        assert!(swamp_core::filter::parse(&p.compose()).is_ok());
    }

    #[test]
    fn seeds_from_current_text() {
        let report = swamp_core::Report {
            store_dir: None,
            observed_at: 0,
            root: "/r".into(),
            projects: vec![],
            unowned: vec![],
            reconciliation: swamp_core::report::Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            notes: vec![],
            series_by_key: Default::default(),
            total_series: Vec::new(),
            series_window_secs: 0,
            dirs_by_worktree: None,
            files_by_worktree: None,
            schedule_line: None,
            summary: Default::default(),
            github_enrichment: None,
            nested_artifacts: Vec::new(),
        };
        let p = Picker::from_report(
            &report,
            "growth < 1GB in 30d idle > 7d merge-complete type:py size > 10MB age > 30d",
            Some(90 * 86_400),
        );
        assert!(!p.growth_gt);
        assert_eq!(SIZES[p.size_ix], "1GB");
        assert_eq!(p.windows[p.window_ix], "30d");
        assert_eq!(IDLES[p.idle_ix], "7d");
        assert!(p.merge_complete);
        assert_eq!(p.types[p.type_ix], "py");
        assert_eq!(SIZES[p.min_size_ix], "10MB");
        assert_eq!(IDLES[p.age_ix], "30d");
        let p0 = Picker::from_report(&report, "0", Some(90 * 86_400));
        assert_eq!(p0.compose(), "0");
    }

    #[test]
    fn tab_completion_is_contextual() {
        let projects = vec![
            "mole".to_string(),
            "memex".to_string(),
            "hiphi-cloud".to_string(),
        ];
        let (t, c) = apply_completion("gr", &projects);
        assert_eq!(t, "growth > ");
        assert_eq!(c.len(), 1);
        let (t, _) = apply_completion("growth > 1", &projects);
        assert!(t.starts_with("growth > 1"));
        let (t, c) = apply_completion("project:m", &projects);
        assert_eq!(c.len(), 2);
        assert_eq!(t, "project:m");
        let (t, _) = apply_completion("project:mo", &projects);
        assert_eq!(t, "project:mole");
        let (t, _) = apply_completion("growth > 100MB in ", &projects);
        assert_eq!(t, "growth > 100MB in ");
        let (_, c) = apply_completion("growth > 100MB in 7", &projects);
        assert!(c.contains(&"7d".to_string()));
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn windows_never_exceed_the_history_the_store_holds() {
        // Four hours of observations: a week-long window would report
        // growth the tool never observed.
        let w = windows_for(Some(4 * 3_600 + 600));
        assert_eq!(w, vec!["1h".to_string(), "all history (4h)".to_string()]);
        assert!(!w.iter().any(|x| x == "7d" || x == "1y"));
        assert_eq!(window_secs("all history (4h)", Some(15_000)), 15_000);
        assert_eq!(window_filter_text("all history (4h)", Some(15_000)), "4h");

        // Exactly on a rung: no synthetic entry.
        let w = windows_for(Some(7 * 86_400));
        assert_eq!(w.last().unwrap(), "7d");

        // A month of history offers the whole ladder up to 30d.
        let w = windows_for(Some(30 * 86_400));
        assert!(w.contains(&"24h".to_string()) && w.contains(&"30d".to_string()));
        assert!(!w.contains(&"90d".to_string()));

        // No observations at all.
        assert_eq!(windows_for(None), vec!["all history".to_string()]);
    }
}
