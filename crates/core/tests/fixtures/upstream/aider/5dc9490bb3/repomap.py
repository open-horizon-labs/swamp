# repo: https://github.com/Aider-AI/aider (Apache-2.0)
# commit: 5dc9490bb35f9729ef2c95d00a19ccd30c26339c  committed: 2026-05-22T14:02:20Z  retrieved: 2026-09-22
# path: aider/repomap.py  lines 35-43, 186, 218
CACHE_VERSION = 3
if USING_TSL_PACK:
    CACHE_VERSION = 4

UPDATING_REPO_MAP_MESSAGE = "Updating repo map"


class RepoMap:
    TAGS_CACHE_DIR = f".aider.tags.cache.v{CACHE_VERSION}"
...
        path = Path(self.root) / self.TAGS_CACHE_DIR
        path = Path(self.root) / self.TAGS_CACHE_DIR
