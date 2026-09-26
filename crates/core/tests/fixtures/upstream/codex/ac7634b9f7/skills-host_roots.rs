// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/ext/skills/src/host_roots.rs
// retrieved: 2026-09-22
// --- lines 24-25: consts ---
const AGENTS_DIR_NAME: &str = ".agents";
const SKILLS_DIR_NAME: &str = "skills";
// --- lines 95-108: $CODEX_HOME/skills DEPRECATED, ~/.agents/skills is current ---
            ConfigLayerSource::User { .. } => {
                // Deprecated user skills location (`$CODEX_HOME/skills`), kept for backward
                // compatibility.
                roots.push(local_root(
                    config_folder.join(SKILLS_DIR_NAME),
                    SkillScope::User,
                ));

                if let Some(home_dir) = home_dir {
                    roots.push(local_root(
                        home_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME),
                        SkillScope::User,
                    ));
                }
