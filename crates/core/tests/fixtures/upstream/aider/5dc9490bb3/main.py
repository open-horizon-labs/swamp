# repo: https://github.com/Aider-AI/aider (Apache-2.0)
# commit: 5dc9490bb35f9729ef2c95d00a19ccd30c26339c  committed: 2026-05-22T14:02:20Z  retrieved: 2026-09-22
# path: aider/main.py  lines 464-476
    conf_fname = Path(".aider.conf.yml")

    default_config_files = []
    try:
        default_config_files += [conf_fname.resolve()]  # CWD
    except OSError:
        pass

    if git_root:
        git_conf = Path(git_root) / conf_fname  # git root
        if git_conf not in default_config_files:
            default_config_files.append(git_conf)
    default_config_files.append(Path.home() / conf_fname)  # homedir
