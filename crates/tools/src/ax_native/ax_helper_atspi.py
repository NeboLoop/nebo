#!/usr/bin/env python3
"""Nebo accessibility helper (Linux): AT-SPI2 walk in the shared JSON-lines format.

  tree --app NAME [--window 1] [--depth 12] [--max 400] [--timeout-ms 2000]
  act  --app NAME --path 0.3.2 --action AXPress [--window 1]
  set  --app NAME --path 0.3.2 --value TEXT [--window 1]
  selftest

Wire format (one JSON object per line, see ax_native/mod.rs):
  {"app":..,"pid":..,"windows":..}
  {"path":"0.3.2","role":"AXButton","title":..,"value":..,"desc":..,
   "frame":[x,y,w,h],"actions":["AXPress"],"enabled":true,"focused":false}
  {"truncated":false,"elapsed_ms":210}
Roles and actions use the macOS AX vocabulary on every platform.
"""
import argparse
import json
import sys
import time
import warnings

# gi flips which of get_component()/get_component_iface() (and the action
# pair) it marks deprecated between at-spi2 releases; both work on every
# release we ship. stderr is this helper's failure channel, so the
# notices must not land there (seen live 2026-09-20, bookworm).
warnings.simplefilter("ignore", DeprecationWarning)

# AT-SPI role name (Atspi.Accessible.get_role_name) -> AX vocabulary.
ROLE_MAP = {
    "push button": "AXButton",
    "toggle button": "AXButton",
    "button": "AXButton",
    "check box": "AXCheckBox",
    "radio button": "AXRadioButton",
    "entry": "AXTextField",
    "password text": "AXTextField",
    "link": "AXLink",
    "label": "AXStaticText",
    "heading": "AXHeading",
    "image": "AXImage",
    "icon": "AXImage",
    "menu item": "AXMenuItem",
    "check menu item": "AXMenuItem",
    "radio menu item": "AXMenuItem",
    "menu": "AXMenu",
    "menu bar": "AXMenuBar",
    "combo box": "AXComboBox",
    "slider": "AXSlider",
    "spin button": "AXSlider",
    "page tab": "AXTab",
    "list": "AXList",
    "list item": "AXStaticText",
    "table": "AXTable",
    "tree": "AXOutline",
    "tree table": "AXOutline",
    "panel": "AXGroup",
    "filler": "AXGroup",
    "section": "AXGroup",
    "viewport": "AXGroup",
    "scroll pane": "AXGroup",
    "frame": "AXWindow",
    "dialog": "AXWindow",
    "window": "AXWindow",
}
WINDOW_ROLES = ("frame", "dialog", "window")
PRESS_ACTIONS = ("click", "press", "activate", "jump")
GROUP_ROLE = "AXGroup"


def ax_role(role_name, editable=False, multiline=False):
    """Map an AT-SPI role name to the AX vocabulary.

    `text` depends on state: editable -> field/area, otherwise static text.
    Unknown roles become AX + CamelCase so nothing is silently dropped.
    """
    r = (role_name or "").strip().lower()
    if r == "text":
        if not editable:
            return "AXStaticText"
        return "AXTextArea" if multiline else "AXTextField"
    if r in ROLE_MAP:
        return ROLE_MAP[r]
    return "AX" + "".join(w.capitalize() for w in r.split()) if r else "AXUnknown"


def ax_actions(action_names, editable=False):
    """AT-SPI action names -> AX action names, order kept, no duplicates."""
    out = []
    for name in action_names:
        n = (name or "").strip().lower()
        if n in PRESS_ACTIONS and "AXPress" not in out:
            out.append("AXPress")
    if editable and "AXSetValue" not in out:
        out.append("AXSetValue")
    return out


def parse_path(path):
    """'0.3.2' -> [0, 3, 2]; anything else is an error."""
    try:
        parts = [int(p) for p in str(path).split(".")]
    except ValueError:
        raise SystemExit(f"bad path {path!r}: expected child indices like 0.3.2")
    if not parts or any(p < 0 for p in parts):
        raise SystemExit(f"bad path {path!r}: expected child indices like 0.3.2")
    return parts


def intersects(a, b):
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    return aw > 0 and ah > 0 and ax < bx + bw and ax + aw > bx and ay < by + bh and ay + ah > by


# --- AT-SPI (loaded lazily so py_compile/selftest work without gi) ----------


def load_atspi():
    try:
        import gi

        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
    except Exception as e:  # ImportError, ValueError from require_version
        sys.stderr.write(
            f"AT-SPI unavailable: {e} (needs python3-gi and gir1.2-atspi-2.0, and an AT-SPI bus on DISPLAY)\n"
        )
        sys.exit(2)
    try:
        Atspi.init()
    except Exception:
        pass
    return Atspi


def find_app(Atspi, name):
    desktop = Atspi.get_desktop(0)
    want = name.strip().lower()
    apps = [desktop.get_child_at_index(i) for i in range(desktop.get_child_count())]
    apps = [a for a in apps if a is not None]
    for a in apps:
        if (a.get_name() or "").lower() == want:
            return a
    for a in apps:
        if want in (a.get_name() or "").lower():
            return a
    names = ", ".join(sorted({a.get_name() or "?" for a in apps}))
    raise SystemExit(f"no application named {name!r} on the accessibility bus (have: {names})")


def app_windows(app):
    wins = []
    for i in range(app.get_child_count()):
        c = app.get_child_at_index(i)
        if c is not None and (c.get_role_name() or "").lower() in WINDOW_ROLES:
            wins.append(c)
    return wins


def resolve(Atspi, app_name, window_idx):
    app = find_app(Atspi, app_name)
    wins = app_windows(app)
    if not wins:
        raise SystemExit(f"{app.get_name()!r} has no window")
    if window_idx < 1 or window_idx > len(wins):
        raise SystemExit(f"{app.get_name()!r} has {len(wins)} window(s); --window {window_idx} is out of range")
    return app, wins, wins[window_idx - 1]


def node_at(window, path):
    el = window
    for idx in parse_path(path):
        if idx >= el.get_child_count():
            raise SystemExit(f"path {path}: no child {idx} (walk the tree again; the window changed)")
        el = el.get_child_at_index(idx)
    return el


def extents(Atspi, el):
    try:
        comp = el.get_component_iface()
        if comp is None:
            return None
        r = comp.get_extents(Atspi.CoordType.SCREEN)
        return [int(r.x), int(r.y), int(r.width), int(r.height)]
    except Exception:
        return None


def has_state(Atspi, el, name):
    try:
        return el.get_state_set().contains(getattr(Atspi.StateType, name))
    except Exception:
        return False


def action_names(el):
    try:
        act = el.get_action_iface()
        if act is None:
            return []
        n = act.get_n_actions()
        getter = getattr(act, "get_action_name", None) or getattr(act, "get_name")
        return [getter(i) or "" for i in range(n)]
    except Exception:
        return []


def text_value(el):
    try:
        t = el.get_text_iface()
        if t is not None:
            return t.get_text(0, t.get_character_count())
    except Exception:
        pass
    try:
        v = el.get_value_iface()
        if v is not None:
            return str(v.get_current_value())
    except Exception:
        pass
    return None


def cmd_tree(args):
    Atspi = load_atspi()
    start = time.monotonic()
    app, wins, window = resolve(Atspi, args.app, args.window)
    print(json.dumps({"app": app.get_name() or args.app, "pid": app.get_process_id(), "windows": len(wins)}))
    win_rect = extents(Atspi, window) or [0, 0, 1 << 30, 1 << 30]
    emitted = 0
    truncated = False
    # Explicit stack: (element, path, depth); children pushed reversed so order is kept.
    stack = []
    for i in range(window.get_child_count() - 1, -1, -1):
        stack.append((window.get_child_at_index(i), str(i), 1))
    while stack:
        if (time.monotonic() - start) * 1000 > args.timeout_ms or emitted >= args.max:
            truncated = True
            break
        el, path, depth = stack.pop()
        if el is None:
            continue
        editable = has_state(Atspi, el, "EDITABLE")
        role = ax_role(el.get_role_name(), editable, has_state(Atspi, el, "MULTI_LINE"))
        title = el.get_name() or ""
        frame = extents(Atspi, el)
        visible = frame is not None and intersects(frame, win_rect) and has_state(Atspi, el, "SHOWING")
        flatten = role == GROUP_ROLE and not title
        if visible and not flatten:
            print(json.dumps({
                "path": path,
                "role": role,
                "title": title,
                "value": text_value(el) if (editable or role in ("AXTextField", "AXTextArea", "AXSlider", "AXComboBox")) else None,
                "desc": el.get_description() or None,
                "frame": frame,
                "actions": ax_actions(action_names(el), editable),
                "enabled": has_state(Atspi, el, "ENABLED") or has_state(Atspi, el, "SENSITIVE"),
                "focused": has_state(Atspi, el, "FOCUSED"),
            }))
            emitted += 1
        if depth >= args.depth:
            continue
        try:
            n = el.get_child_count()
        except Exception:
            n = 0
        for i in range(n - 1, -1, -1):
            stack.append((el.get_child_at_index(i), f"{path}.{i}", depth + 1))
    print(json.dumps({"truncated": truncated, "elapsed_ms": int((time.monotonic() - start) * 1000)}))


def cmd_act(args):
    Atspi = load_atspi()
    _, _, window = resolve(Atspi, args.app, args.window)
    el = node_at(window, args.path)
    act = el.get_action_iface()
    names = [n.lower() for n in action_names(el)]
    if act is None or not names:
        raise SystemExit(f"{el.get_role_name()} {el.get_name()!r} has no accessibility actions; click it by coordinate")
    want = args.action.strip()
    if want == "AXPress":
        idx = next((i for i, n in enumerate(names) if n in PRESS_ACTIONS), 0)
    elif want.lower() in names:
        idx = names.index(want.lower())
    else:
        raise SystemExit(f"{el.get_role_name()} {el.get_name()!r} does not support {want}; it has: {', '.join(names)}")
    if not act.do_action(idx):
        raise SystemExit(f"{names[idx]} on {el.get_name()!r} returned false")


def cmd_set(args):
    Atspi = load_atspi()
    _, _, window = resolve(Atspi, args.app, args.window)
    el = node_at(window, args.path)
    et = el.get_editable_text_iface()
    if et is not None:
        if not et.set_text_contents(args.value):
            raise SystemExit(f"could not set text on {el.get_name()!r}")
        return
    v = el.get_value_iface()
    if v is not None:
        try:
            v.set_current_value(float(args.value))
            return
        except ValueError:
            raise SystemExit(f"{el.get_name()!r} takes a number, not {args.value!r}")
    raise SystemExit(f"{el.get_role_name()} {el.get_name()!r} is not editable")


def selftest():
    assert ax_role("push button") == "AXButton"
    assert ax_role("Toggle Button") == "AXButton"
    assert ax_role("entry", editable=True) == "AXTextField"
    assert ax_role("text") == "AXStaticText"
    assert ax_role("text", editable=True) == "AXTextField"
    assert ax_role("text", editable=True, multiline=True) == "AXTextArea"
    assert ax_role("scroll pane") == "AXGroup"
    assert ax_role("dialog") == "AXWindow"
    assert ax_role("list item") == "AXStaticText"
    assert ax_role("color chooser") == "AXColorChooser"
    assert ax_role("") == "AXUnknown"
    assert ax_actions(["click", "press"]) == ["AXPress"]
    assert ax_actions(["activate"], editable=True) == ["AXPress", "AXSetValue"]
    assert ax_actions(["expand"]) == []
    assert ax_actions([], editable=True) == ["AXSetValue"]
    assert parse_path("0.3.2") == [0, 3, 2]
    assert parse_path("7") == [7]
    for bad in ("", "a.b", "1.-2", "1..2"):
        try:
            parse_path(bad)
        except SystemExit:
            pass
        else:
            raise AssertionError(f"parse_path accepted {bad!r}")
    assert intersects([10, 10, 5, 5], [0, 0, 100, 100])
    assert not intersects([200, 10, 5, 5], [0, 0, 100, 100])
    assert not intersects([10, 10, 0, 5], [0, 0, 100, 100])
    print("ok")


def main(argv):
    p = argparse.ArgumentParser(prog="ax_helper_atspi")
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("tree", "act", "set"):
        s = sub.add_parser(name)
        s.add_argument("--app", required=True)
        s.add_argument("--window", type=int, default=1)
        if name == "tree":
            s.add_argument("--depth", type=int, default=12)
            s.add_argument("--max", type=int, default=400)
            s.add_argument("--timeout-ms", type=int, default=2000)
        else:
            s.add_argument("--path", required=True)
        if name == "act":
            s.add_argument("--action", required=True)
        if name == "set":
            s.add_argument("--value", required=True)
    sub.add_parser("selftest")
    args = p.parse_args(argv)
    if args.cmd == "selftest":
        selftest()
    elif args.cmd == "tree":
        cmd_tree(args)
    elif args.cmd == "act":
        cmd_act(args)
    else:
        cmd_set(args)


if __name__ == "__main__":
    main(sys.argv[1:])
