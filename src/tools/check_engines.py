"""Compares the engine versions pinned in engines.json with the latest release of each engine on GitHub.

Run before every build (rule of 3/11/2026): `python tools/check_engines.py`. Read only; it changes nothing.
Prints one line per engine: pinned tag, latest tag, whether the latest is listed in engine-compat.json
(tested against our apps) and whether the pinned asset name exists for the latest release. It needs `gh`."""
import json
import re
import subprocess
import sys
from pathlib import Path

here = Path(__file__).resolve().parent.parent
pins = json.loads((here / "engines.json").read_text(encoding="utf-8"))["engines"]
compat = json.loads((here / "engine-compat.json").read_text(encoding="utf-8"))


def latest(repo):
    out = subprocess.run(["gh", "release", "view", "-R", repo, "--json", "tagName,publishedAt,assets"],
                         capture_output=True, text=True)
    if out.returncode != 0:
        return None, None, [], out.stderr.strip()
    data = json.loads(out.stdout)
    return data["tagName"], data["publishedAt"][:10], [a["name"] for a in data["assets"]], ""


def workarounds_for(name):
    """The headings of ENGINE-WORKAROUNDS.md that belong to this engine (`## <Engine>: ...`)."""
    path = here / "ENGINE-WORKAROUNDS.md"
    if not path.is_file():
        return []
    return [l[3:].strip() for l in path.read_text(encoding="utf-8").splitlines()
            if l.startswith("## ") and l[3:].lower().startswith(name.lower() + ":")]


behind = 0
newer_engines = []
for e in pins:
    tag, date, assets, err = latest(e["repo"])
    if tag is None:
        print(f"{e['name']:<11} pinned {e['tag']:<8} could not read the latest release: {err}")
        continue
    newer = tag != e["tag"]
    behind += newer
    if newer:
        newer_engines.append(e["name"])
    ver = tag.lstrip("v")
    asset = re.sub(r"\d+\.\d+\.\d+", ver, e["asset"])
    has_asset = asset in assets
    tested = ver in compat.get(e["name"], [])
    state = "up to date" if not newer else "NEWER AVAILABLE"
    print(f"{e['name']:<11} pinned {e['tag']:<8} latest {tag:<8} ({date}) {state:<16}"
          f" asset {'found' if has_asset else 'MISSING'}, in engine-compat: {'yes' if tested else 'no'}")
print(f"\n{behind} of {len(pins)} engines have a newer release than the pinned one.")
# Rule of 22/11/2026: we fix some gaps of the engines ourselves; a new engine may fix them, and then we must not keep
# both. For every engine that is behind, say which of our fixes to check against its release notes and drop if it is done.
for name in newer_engines:
    mine = workarounds_for(name)
    if mine:
        print(f"\nBefore aligning {name}: does the new release fix one of OUR workarounds? (ENGINE-WORKAROUNDS.md)")
        for m in mine:
            print(f"  - {m}")
sys.exit(0)
