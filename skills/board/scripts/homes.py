"""Agent homes on this machine, and what the board puts in them.

Claude Code and Codex each keep an account's settings in a home: ~/.claude or ~/.codex, or a
folder named after one with a dash (~/.claude-work), which CLAUDE_CONFIG_DIR or CODEX_HOME then
names. A machine may hold several of each. `found` lists them, `plan` says what the board's skill
and its session-start notice need in each, and `checks` is what `board doctor` reports.

What reads what, as tried with Claude Code 2.1 and Codex 0.171: Codex reads ~/.agents/skills
under every home; Claude Code reads only the skills folder of the home it runs under; and each
runs a session-start hook from a file of the home (settings.json, hooks.json). Codex asks its
user to trust a new hook before it runs it, and nothing here answers that for them.
"""

import json
import os
import re
import shutil
import stat
from collections import namedtuple

Home = namedtuple("Home", "agent path")
Step = namedtuple("Step", "what path act")

MARKS = {
    "claude": ("settings.json", "projects"),
    "codex": ("config.toml", "auth.json", os.path.join("skills", ".system")),
}
VARIABLE = {"claude": "CLAUDE_CONFIG_DIR", "codex": "CODEX_HOME"}
HOOK_FILE = {"claude": "settings.json", "codex": "hooks.json"}
INSTALLED = os.path.join(".agents", "skills", "board")
SCRIPT = os.path.join(INSTALLED, "scripts", "board")


class Unusable(Exception):
    """Something of a home, or in the way of one, that is not what it should be."""


def found(top=None, environ=None):
    """This user's homes of both agents: for each, the default folder and those named after it
    with a dash, when they hold what a home holds, and the home this command runs under when it
    is elsewhere. It is the convention the agents' own variables follow, so a home made later is
    found the next time anything looks."""
    top = top or os.path.expanduser("~")
    environ = os.environ if environ is None else environ
    homes = []
    for agent in ("claude", "codex"):
        base = f".{agent}"
        names = sorted(name for name in os.listdir(top) if name == base or name.startswith(f"{base}-"))
        paths = [
            path for path in (os.path.join(top, name) for name in names)
            if any(os.path.exists(os.path.join(path, mark)) for mark in MARKS[agent])
        ]
        here = environ.get(VARIABLE[agent])
        if here and os.path.isdir(here) and os.path.abspath(here) not in paths:
            paths.append(os.path.abspath(here))
        homes += [Home(agent, path) for path in paths]
    return homes


def is_skill(folder):
    """Whether `folder` holds the board's skill: a SKILL.md that names the skill board."""
    try:
        with open(os.path.join(folder, "SKILL.md"), encoding="utf-8") as skill:
            return re.search(r"(?m)^name:\s*board\s*$", skill.read()) is not None
    except (OSError, ValueError):
        return False


def made_here(link, target):
    """Whether `link` is a symbolic link to `target`: one `plan` made."""
    return os.path.islink(link) and os.path.realpath(link) == os.path.realpath(target)


def hook_entry(agent):
    """The session-start hook of `agent`, as both agents' files spell one. It names the script
    by the user's home, which the shell that runs a hook fills in, so the same line serves every
    machine."""
    command = f'"$HOME/{SCRIPT}" hook {agent}'
    return {"matcher": "startup|resume", "hooks": [{"type": "command", "command": command, "timeout": 20}]}


def is_ours(entry):
    """Whether `entry`, one of a file's SessionStart hooks, runs the board's script."""
    handlers = entry.get("hooks") if isinstance(entry, dict) else None
    return isinstance(handlers, list) and any(
        isinstance(handler, dict) and f"/{SCRIPT}\" hook " in str(handler.get("command", ""))
        for handler in handlers
    )


def session_starts(document):
    """The SessionStart hooks of `document` (a settings.json or a hooks.json, read), as the list
    it holds or would hold."""
    hooks = document.get("hooks", {})
    if not isinstance(hooks, dict):
        raise Unusable("its hooks are not an object")
    starts = hooks.get("SessionStart", [])
    if not isinstance(starts, list):
        raise Unusable("its SessionStart hooks are not a list")
    return starts


def has_hook(document):
    return any(is_ours(entry) for entry in session_starts(document))


def with_hook(document, agent):
    """`document` with the board's session-start hook of `agent`, once, after the hooks it
    already has. Whatever else the file holds stays as it is, in its order."""
    starts = session_starts(document)
    if [entry for entry in starts if is_ours(entry)] == [hook_entry(agent)]:
        return document
    others = [entry for entry in starts if not is_ours(entry)]
    hooks = dict(document.get("hooks", {}), SessionStart=[*others, hook_entry(agent)])
    return dict(document, hooks=hooks)


def without_hook(document):
    """`document` without the board's session-start hook and without the lists that held nothing
    else, in its order; a document that never had the hook, as it is."""
    if not has_hook(document):
        return document
    others = [entry for entry in session_starts(document) if not is_ours(entry)]
    hooks = {
        event: others if event == "SessionStart" else held
        for event, held in document["hooks"].items() if event != "SessionStart" or others
    }
    return {key: hooks if key == "hooks" else value for key, value in document.items() if key != "hooks" or hooks}


def read_json(path):
    """The JSON object at `path`, or {} when no file is there."""
    try:
        with open(path, encoding="utf-8") as file:
            document = json.load(file)
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as error:
        raise Unusable(f"cannot be read as JSON ({error})") from error
    if not isinstance(document, dict):
        raise Unusable("does not hold a JSON object")
    return document


def write_json(path, document):
    """Writes `document` at `path` as the agents write theirs (two spaces, a last newline): whole
    or not at all, into the file a symbolic link names, and with the mode the file had."""
    target = os.path.realpath(path)
    partial = f"{target}.{os.getpid()}.tmp"
    try:
        with open(partial, "w", encoding="utf-8") as file:
            json.dump(document, file, indent=2, ensure_ascii=False)
            file.write("\n")
        if os.path.exists(target):
            os.chmod(partial, stat.S_IMODE(os.stat(target).st_mode))
        os.replace(partial, target)
    finally:
        if os.path.exists(partial):
            os.unlink(partial)


def copy_skill(source, folder):
    """Copies the skill from `source` to `folder`: its SKILL.md and its scripts, without their
    tests. A copy, never a link into a checkout, whose files change with its branch."""
    scripts = os.path.join(folder, "scripts")
    if os.path.isdir(scripts):
        shutil.rmtree(scripts)
    os.makedirs(scripts)
    shutil.copyfile(os.path.join(source, "SKILL.md"), os.path.join(folder, "SKILL.md"))
    for name in sorted(os.listdir(os.path.join(source, "scripts"))):
        path = os.path.join(source, "scripts", name)
        if os.path.isfile(path) and not name.startswith("test_") and not name.endswith(".pyc"):
            shutil.copyfile(path, os.path.join(scripts, name))
    os.chmod(os.path.join(scripts, "board"), 0o755)


def same_skill(source, folder):
    """Whether `folder` holds what `copy_skill` would put there from `source` now."""
    names = [os.path.join("scripts", name) for name in sorted(os.listdir(os.path.join(source, "scripts")))]
    names = [name for name in names if os.path.isfile(os.path.join(source, name))]
    names = [name for name in names if not os.path.basename(name).startswith("test_") and not name.endswith(".pyc")]
    try:
        for name in ["SKILL.md", *names]:
            with open(os.path.join(source, name), "rb") as ours, open(os.path.join(folder, name), "rb") as theirs:
                if ours.read() != theirs.read():
                    return False
    except OSError:
        return False
    return True


def plan(source, hooks, remove=False, top=None, environ=None):
    """What puts the board into every home of this user, or with `remove` takes it out: the
    steps, each with what it does, the path it does it to and how (a step without an action only
    reports), and what was left alone and why.

    The skill's folder is copied to ~/.agents/skills/board, which Codex reads under every home;
    each Claude home's skills get a link to it; ~/.local/bin gets a link to the script; and with
    `hooks` each home's hook file gets the session-start hook, which is taken out without them.
    Nothing that this did not make is overwritten or removed."""
    top = top or os.path.expanduser("~")
    folder = os.path.join(top, INSTALLED)
    script = os.path.join(top, SCRIPT)
    homes = found(top, environ)
    links = [(os.path.join(home.path, "skills", "board"), folder, home.path) for home in homes if home.agent == "claude"]
    links.append((os.path.join(top, ".local", "bin", "board"), script, os.path.join(top, ".local", "bin")))
    installed = os.path.isdir(folder) and not os.path.islink(folder) and is_skill(folder)
    steps, refused = [], []

    def link(path, target):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        os.symlink(target, path)

    if remove:
        for path, target, _ in links:
            if made_here(path, target):
                steps.append(Step("removed", path, lambda path=path: os.unlink(path)))
            elif os.path.lexists(path):
                refused.append(f"{path}: not a link this made, left as it is")
        if installed:
            steps.append(Step("removed", folder, lambda: shutil.rmtree(folder)))
        elif os.path.lexists(folder):
            refused.append(f"{folder}: not a folder this made, left as it is")
    else:
        if os.path.realpath(folder) == os.path.realpath(source):
            steps.append(Step("kept", folder, None))
        elif os.path.lexists(folder) and not installed:
            raise Unusable(f"{folder}: something else is there, nothing was changed")
        elif installed and same_skill(source, folder):
            steps.append(Step("kept", folder, None))
        else:
            steps.append(Step("copied", folder, lambda: copy_skill(source, folder)))
        for path, target, parent in links:
            if made_here(path, target):
                steps.append(Step("kept", path, None))
            elif os.path.lexists(path):
                refused.append(f"{path}: something else is there, left as it is")
            elif not os.path.isdir(parent):
                refused.append(f"{path}: {parent} is not a folder")
            else:
                steps.append(Step("linked", path, lambda path=path, target=target: link(path, target)))
    for home in homes:
        path = os.path.join(home.path, HOOK_FILE[home.agent])
        try:
            document = read_json(path)
            wanted = with_hook(document, home.agent) if hooks and not remove else without_hook(document)
        except Unusable as error:
            refused.append(f"{path}: {error}, left as it is")
            continue
        if wanted == document:
            if hooks and not remove:
                steps.append(Step("kept", path, None))
        elif not wanted:
            # the hook was all the file held, and an empty file says what no file says
            steps.append(Step("removed", path, lambda path=path: os.unlink(path)))
        else:
            what = "hooked" if hooks and not remove else "unhooked"
            steps.append(Step(what, path, lambda path=path, wanted=wanted: write_json(path, wanted)))
    return steps, refused


def sandbox_of(home):
    """How a Codex home's sessions run commands, by its config.toml's own top-level setting, or
    None when it sets none (Codex then keeps commands in its sandbox until its user approves)."""
    try:
        with open(os.path.join(home.path, "config.toml"), encoding="utf-8") as config:
            top = config.read().split("\n[", 1)[0]
    except (OSError, ValueError):
        return None
    mode = re.search(r'(?m)^sandbox_mode\s*=\s*"([^"]*)"', top)
    return mode.group(1) if mode else None


def checks(top=None, environ=None):
    """What each home has of the board, for `board doctor`: per home its agent, its path, how
    its sessions find the skill, whether its session-start hook is set, and for a Codex home how
    it runs commands."""
    top = top or os.path.expanduser("~")
    folder = os.path.join(top, INSTALLED)
    installed = os.path.isdir(folder) and not os.path.islink(folder) and is_skill(folder)
    rows = []
    for home in found(top, environ):
        if home.agent == "claude":
            link = os.path.join(home.path, "skills", "board")
            if made_here(link, folder):
                skill = "linked" if installed else "linked to nothing"
            else:
                skill = "something else is there" if os.path.lexists(link) else "missing"
        else:
            skill = "read from ~/.agents/skills" if installed else "missing"
        try:
            hook = "set" if has_hook(read_json(os.path.join(home.path, HOOK_FILE[home.agent]))) else "missing"
        except Unusable as error:
            hook = f"unreadable: {error}"
        row = {"agent": home.agent, "home": home.path, "skill": skill, "hook": hook}
        if home.agent == "codex":
            row["sandbox"] = sandbox_of(home)
        rows.append(row)
    return rows


def chosen_hooks(chosen):
    """Whether `setup` was last asked for the session-start notice, as the file at `chosen`
    remembers it, or None when setup has not run."""
    try:
        document = read_json(chosen)
    except Unusable:
        return None
    return bool(document["hooks"]) if "hooks" in document else None


def setup(source, hooks, remove, dry_run, chosen, top=None, environ=None):
    """Carries out `plan`, unless `dry_run`, and remembers at `chosen` what was asked, so that
    `heal` can do the same for a home made later. Returns what a command reports: a document,
    its lines for a person, and whether anything was left alone."""
    steps, refused = plan(source, hooks, remove, top, environ)
    if not dry_run:
        for step in steps:
            if step.act:
                step.act()
        if not remove:
            os.makedirs(os.path.dirname(chosen), mode=0o700, exist_ok=True)
            write_json(chosen, {"hooks": hooks})
        elif os.path.exists(chosen):
            os.unlink(chosen)
    would = "would have " if dry_run else ""
    lines = [f"{would if step.act else ''}{step.what:<9}{step.path}" for step in steps]
    lines += [f"skipped  {note}" for note in refused]
    if not remove:
        if any(step.what == "hooked" and step.path.endswith("hooks.json") for step in steps):
            lines.append(
                "Codex asks once, the next time it starts under each of its homes, whether to"
                " trust the new hook, and does not run it before."
            )
        if hooks:
            lines.append("A home made later is set up when the next session starts under any other.")
        else:
            lines.append("A Claude home made later needs setup again.")
    document = {
        "steps": [{"what": step.what, "path": step.path} for step in steps],
        "skipped": refused, "dry_run": dry_run,
    }
    return document, lines, bool(refused)


def heal(chosen, top=None, environ=None):
    """Does for a home made since what `setup` was last asked to do for the others: the same
    steps, only where something is missing, from the copy already installed. Returns the steps
    it took."""
    top = top or os.path.expanduser("~")
    folder = os.path.join(top, INSTALLED)
    hooks = chosen_hooks(chosen)
    if hooks is None or not is_skill(folder):
        return []
    steps, _ = plan(folder, hooks, top=top, environ=environ)
    done = [step for step in steps if step.act]
    for step in done:
        step.act()
    return done


def rows(source, chosen, top=None, environ=None):
    """What `board doctor` says of what is installed: the skill's copy, the command, and each
    home. A row says whether it is as it should be, what it is about, and the text to show."""
    top = top or os.path.expanduser("~")
    folder = os.path.join(top, INSTALLED)
    told = []
    if not is_skill(folder):
        told.append((False, "skill", f"not installed at {folder}: board setup"))
    elif os.path.realpath(folder) == os.path.realpath(source) or same_skill(source, folder):
        told.append((True, "skill", f"{folder}, the same as this copy"))
    else:
        told.append((False, "skill", f"{folder} differs from the copy at {source}: board setup brings it up to date"))
    command, linked = shutil.which("board"), os.path.join(top, ".local", "bin", "board")
    if command:
        told.append((True, "command", command))
    elif os.path.lexists(linked):
        told.append((False, "command", f"{linked} is there, but its folder is not on this shell's PATH"))
    else:
        told.append((False, "command", "board is not on the PATH: board setup links it into ~/.local/bin"))
    wanted = chosen_hooks(chosen)
    for home in checks(top, environ):
        skill_ok = home["skill"] in ("linked", "read from ~/.agents/skills")
        hook_ok = home["hook"] == "set" or (home["hook"] == "missing" and wanted is False)
        text = f"{home['home']}: skill {home['skill']}, session-start notice {home['hook']}"
        if home["agent"] == "codex":
            if home["hook"] == "set":
                text += " (Codex runs it once its user has trusted it)"
            if home.get("sandbox") == "danger-full-access":
                text += ", commands run with full access"
            else:
                text += ", commands stay in Codex's sandbox until approved: board needs the network"
        told.append((skill_ok and hook_ok, home["agent"], text))
    return [{"ok": ok, "what": what, "text": text} for ok, what, text in told]
