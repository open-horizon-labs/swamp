// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/rollout/src/rollout_file_name.rs
// retrieved: 2026-09-22
// --- lines 38-46: parse rollout-<ts>-<ids>.jsonl ---

    pub(crate) fn parse(name: &str) -> Option<Self> {
        let name = compression::parse_rollout_file_name(name)?;
        let core = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
        let timestamp = core.get(..19)?;
        if core.get(19..20)? != "-" {
            return None;
        }
        let ids = core.get(20..)?;
// --- lines 62-73: render ---
    pub(crate) fn render(&self) -> Result<String, time::error::Format> {
        let format: &[FormatItem] =
            format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]");
        let timestamp = self.timestamp.format(format)?;
        Ok(if self.thread_id == self.rollout_id {
            format!("rollout-{timestamp}-{}.jsonl", self.thread_id)
        } else {
            format!(
                "rollout-{timestamp}-{}_{}.jsonl",
                self.thread_id, self.rollout_id
            )
        })
