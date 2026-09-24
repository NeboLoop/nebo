// Nebo accessibility helper — walks an app window's AXUIElement tree.
// Compile: swiftc -O -framework ApplicationServices -framework AppKit -o ax-helper ax_helper.swift
//
//   ax-helper tree --app <name|bundle id|pid> [--window 1] [--depth 30] [--max 400] [--timeout-ms 2000]
//   ax-helper act  --app … [--window 1] --path 0.3.2 --action AXPress
//   ax-helper set  --app … [--window 1] --path 0.3.2 --value "text"
//   ax-helper menu --app … --path "File > Export…"      (press a menu bar item)
//   ax-helper menu-list --app … [--path "File"]         (the bar, or one menu's items)
//   ax-helper show-menu --app … --path … [--role --label] (open a context menu, list it)
//   ax-helper scroll-to --app … --path … [--role --label]
//   ax-helper wait --app … --for appears|gone|text|value|menu|menu-closed|window [--role --label --text --value --title] [--timeout-ms 5000]
//   ax-helper click|move|scroll|drag|key|type|hit …      (physical input via CGEvent; no --app)
//
// A path that starts with "m:" is inside the menu open right now (a context
// menu or a menu bar menu), not the window.
//
// Output (tree): JSON lines — header {"app","pid","windows"}, one node per
// line, footer {"truncated","elapsed_ms"}. Frames are screen points, origin
// top-left. `path` is the child-index path from the window over the RAW
// child list, so act/set can re-walk it. Errors go to stderr; exit 1 (bad
// request), 2 (usage), 3 (no accessibility permission).

import AppKit
import Vision
import ApplicationServices
import Foundation

let out = FileHandle.standardOutput
func emit(_ obj: [String: Any]) {
    // ponytail: JSONSerialization per line; a walk is ≤400 lines.
    if let d = try? JSONSerialization.data(withJSONObject: obj, options: [.withoutEscapingSlashes]) {
        out.write(d); out.write("\n".data(using: .utf8)!)
    }
}
func fail(_ msg: String, _ code: Int32 = 1) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(code)
}

let args = CommandLine.arguments
guard args.count >= 2 else { fail("usage: ax-helper tree|act|set --app <name|bundle|pid> … | text --image <file>", 2) }
let command = args[1]
var params: [String: String] = [:]
var i = 2
while i < args.count {
    if args[i].hasPrefix("--"), i + 1 < args.count { params[String(args[i].dropFirst(2))] = args[i + 1]; i += 2 } else { i += 1 }
}
// MARK: - text: every line of text in an image, boxes in the image's own pixels.
// Needs no app and no Accessibility permission, so it runs before those guards.
if command == "text" {
    guard let path = params["image"], let img = NSImage(contentsOfFile: path),
          let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else { fail("text needs --image <readable image file>", 2) }
    let (w, h) = (Double(cg.width), Double(cg.height))
    let req = VNRecognizeTextRequest()
    req.recognitionLevel = .accurate
    req.usesLanguageCorrection = true
    do { try VNImageRequestHandler(cgImage: cg, options: [:]).perform([req]) } catch { fail("text recognition failed: \(error)") }
    for obs in req.results ?? [] {
        guard let top = obs.topCandidates(1).first else { continue }
        let b = obs.boundingBox // normalized, origin bottom-left
        emit(["text": top.string,
              "frame": [Int((b.minX * w).rounded()), Int(((1 - b.maxY) * h).rounded()), Int((b.width * w).rounded()), Int((b.height * h).rounded())],
              "confidence": Double(top.confidence)])
    }
    exit(0)
}

// MARK: - frontmost: the app in front, without System Events (which needs an
// Automation grant the app may not have). Runs before the app guard.
if command == "frontmost" {
    let a = NSWorkspace.shared.frontmostApplication
    emit(["app": a?.localizedName ?? "", "pid": Int(a?.processIdentifier ?? 0), "bundle": a?.bundleIdentifier ?? ""])
    exit(0)
}

// MARK: - Physical input through CGEvent — no cliclick (not on a stock Mac),
// no System Events (needs an Automation grant nobody may be there to click).
// Posting events needs the same Accessibility grant as reading the tree.
let inputCommands: Set<String> = ["click", "move", "scroll", "drag", "key", "type", "hit"]
if inputCommands.contains(command) {
    guard AXIsProcessTrusted() else {
        fail("Accessibility permission is off for this process (System Settings › Privacy & Security › Accessibility)", 3)
    }
    let src = CGEventSource(stateID: .hidSystemState)
    func num(_ k: String) -> Double? { params[k].flatMap { Double($0) } }
    func pt(_ xk: String, _ yk: String) -> CGPoint {
        guard let x = num(xk), let y = num(yk) else { fail("\(command) needs --\(xk) and --\(yk)", 2) }
        return CGPoint(x: x, y: y)
    }
    func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap) }
    func mouse(_ type: CGEventType, _ p: CGPoint, _ button: CGMouseButton, clicks: Int64 = 1) {
        let e = CGEvent(mouseEventSource: src, mouseType: type, mouseCursorPosition: p, mouseButton: button)
        e?.setIntegerValueField(.mouseEventClickState, value: clicks)
        post(e)
    }
    func flags(_ mods: String) -> CGEventFlags {
        var f: CGEventFlags = []
        for m in mods.lowercased().split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) {
            switch m {
            case "cmd", "command": f.insert(.maskCommand)
            case "shift": f.insert(.maskShift)
            case "opt", "option", "alt": f.insert(.maskAlternate)
            case "ctrl", "control": f.insert(.maskControl)
            case "fn": f.insert(.maskSecondaryFn)
            case "": break
            default: fail("unknown modifier \(m)", 2)
            }
        }
        return f
    }
    switch command {
    case "click":
        let p = pt("x", "y")
        let right = params["button"] == "right"
        let (down, up, btn): (CGEventType, CGEventType, CGMouseButton) =
            right ? (.rightMouseDown, .rightMouseUp, .right) : (.leftMouseDown, .leftMouseUp, .left)
        let count = max(1, min(3, Int(params["count"] ?? "1") ?? 1))
        mouse(.mouseMoved, p, btn)
        usleep(30_000)
        for n in 1...count {
            mouse(down, p, btn, clicks: Int64(n)); usleep(20_000)
            mouse(up, p, btn, clicks: Int64(n)); usleep(40_000)
        }
    case "move":
        mouse(.mouseMoved, pt("x", "y"), .left)
    case "scroll":
        // --dy > 0 reveals what is below, --dx > 0 what is to the right; points.
        // Without --x/--y the wheel turns wherever the pointer already is.
        if num("x") != nil { mouse(.mouseMoved, pt("x", "y"), .left); usleep(30_000) }
        var dy = Int32(num("dy") ?? 0), dx = Int32(num("dx") ?? 0)
        guard dx != 0 || dy != 0 else { fail("scroll needs a non-zero --dx or --dy", 2) }
        // Chunks of at most 60 points: apps that coalesce wheel events still move.
        while dx != 0 || dy != 0 {
            let sy = max(-60, min(60, dy)), sx = max(-60, min(60, dx))
            post(CGEvent(scrollWheelEvent2Source: src, units: .pixel, wheelCount: 2, wheel1: -sy, wheel2: -sx, wheel3: 0))
            dy -= sy; dx -= sx
            usleep(12_000)
        }
    case "drag":
        // Press, hold so the item is picked up, move in steps, dwell over the
        // target so it activates (lists, Finder, docks), release.
        let a = pt("x", "y"), b = pt("to-x", "to-y")
        let duration = num("duration-ms") ?? 300, pickup = num("pickup-ms") ?? 200, dwell = num("drop-ms") ?? 500
        mouse(.mouseMoved, a, .left); usleep(30_000)
        mouse(.leftMouseDown, a, .left)
        usleep(useconds_t(pickup * 1000))
        let steps = max(2, Int((duration / 16).rounded(.up)))
        for step in 1...steps {
            let t = Double(step) / Double(steps)
            mouse(.leftMouseDragged, CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t), .left)
            usleep(useconds_t(duration * 1000 / Double(steps)))
        }
        var waited = 0.0
        while waited < dwell { mouse(.leftMouseDragged, b, .left); usleep(16_000); waited += 16 }
        mouse(.leftMouseUp, b, .left)
    case "key":
        guard let code = params["code"].flatMap({ CGKeyCode($0) }) else { fail("key needs --code <virtual key code>", 2) }
        let f = flags(params["mods"] ?? "")
        let down = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true)
        let up = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false)
        down?.flags = f; up?.flags = f
        post(down); usleep(15_000); post(up)
    case "type":
        // Unicode events a few characters at a time: no key-code mapping, and
        // the keyboard layout cannot change what arrives.
        guard let text = params["text"] else { fail("type needs --text", 2) }
        let units = Array(text.utf16)
        var at = 0
        while at < units.count {
            var chunk = Array(units[at..<min(at + 16, units.count)])
            for keyDown in [true, false] {
                let e = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: keyDown)
                e?.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: &chunk)
                post(e)
            }
            at += 16
            usleep(8_000)
        }
    default: // "hit"
        // What a point on screen belongs to, so a pixel click can say what it
        // is about to hit.
        let p = pt("x", "y")
        var hit: AXUIElement?
        guard AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(), Float(p.x), Float(p.y), &hit) == .success,
              let el = hit else { emit(["role": "", "label": "", "app": ""]); exit(0) }
        var pid: pid_t = 0
        AXUIElementGetPid(el, &pid)
        var obj: [String: Any] = ["role": str(el, kAXRoleAttribute) ?? "", "label": identity(el),
                                  "app": NSRunningApplication(processIdentifier: pid)?.localizedName ?? ""]
        if let f = frame(el) { obj["frame"] = [Int(f.minX), Int(f.minY), Int(f.width), Int(f.height)] }
        emit(obj)
    }
    exit(0)
}

guard let appSpec = params["app"], !appSpec.isEmpty else { fail("--app is required", 2) }

guard AXIsProcessTrusted() else {
    fail("Accessibility permission is off for this process (System Settings › Privacy & Security › Accessibility)", 3)
}

// MARK: - App resolution: exact name, case-insensitive name, bundle id, pid.
func resolveApp(_ spec: String) -> NSRunningApplication? {
    // Chromium runs several processes under one name; only the one with a
    // Dock presence (.regular) owns windows.
    let apps = NSWorkspace.shared.runningApplications.sorted { $0.activationPolicy == .regular && $1.activationPolicy != .regular }
    if let a = apps.first(where: { $0.localizedName == spec }) { return a }
    if let a = apps.first(where: { $0.localizedName?.caseInsensitiveCompare(spec) == .orderedSame }) { return a }
    if let a = apps.first(where: { $0.bundleIdentifier == spec }) { return a }
    if let pid = Int32(spec), let a = NSRunningApplication(processIdentifier: pid) { return a }
    return nil
}
guard let app = resolveApp(appSpec) else { fail("no running application matches \"\(appSpec)\"") }
let appEl = AXUIElementCreateApplication(app.processIdentifier)
// A hung app must not hang the walk: cap every AX round-trip.
AXUIElementSetMessagingTimeout(appEl, 0.3)
// Chromium/Electron only build their tree when an assistive client asks.
AXUIElementSetAttributeValue(appEl, "AXManualAccessibility" as CFString, kCFBooleanTrue)

// MARK: - Attribute helpers
func attr(_ el: AXUIElement, _ name: String) -> AnyObject? {
    var v: CFTypeRef?
    return AXUIElementCopyAttributeValue(el, name as CFString, &v) == .success ? v : nil
}
func str(_ el: AXUIElement, _ name: String) -> String? {
    guard let v = attr(el, name) else { return nil }
    if let s = v as? String { return s }
    if let s = v as? NSAttributedString { return s.string }
    if let n = v as? NSNumber { return n.stringValue }
    if let u = v as? URL { return u.absoluteString }
    return nil
}
func bool(_ el: AXUIElement, _ name: String, _ dflt: Bool) -> Bool {
    (attr(el, name) as? Bool) ?? dflt
}
func children(_ el: AXUIElement) -> [AXUIElement] {
    (attr(el, kAXChildrenAttribute) as? [AXUIElement]) ?? []
}
func frame(_ el: AXUIElement) -> CGRect? {
    guard let p = attr(el, kAXPositionAttribute), let s = attr(el, kAXSizeAttribute),
          CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID() else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero
    guard AXValueGetValue(p as! AXValue, .cgPoint, &pt), AXValueGetValue(s as! AXValue, .cgSize, &sz) else { return nil }
    return CGRect(origin: pt, size: sz)
}
func windows() -> [AXUIElement] {
    // Right after AXManualAccessibility is switched on, Chromium/Electron
    // report no windows for a moment; give the tree a short time to appear.
    for attempt in 0..<6 {
        let wins = (attr(appEl, kAXWindowsAttribute) as? [AXUIElement]) ?? []
        if !wins.isEmpty { return wins }
        if attempt < 5 { usleep(50_000) }
    }
    // Chromium lists no AXWindows for a window on another Space yet still
    // answers AXMainWindow (observed: Brave, 2026-09-19). That window is
    // window 1 then.
    for name in [kAXMainWindowAttribute, kAXFocusedWindowAttribute] {
        if let w = attr(appEl, name), CFGetTypeID(w) == AXUIElementGetTypeID() { return [w as! AXUIElement] }
    }
    return []
}
func window(_ index: Int) -> (AXUIElement, Int) {
    let wins = windows()
    guard index >= 1, index <= wins.count else {
        if wins.isEmpty { fail("\(app.localizedName ?? appSpec) has no open window") }
        fail("\(app.localizedName ?? appSpec) has \(wins.count) window(s); there is no window \(index)")
    }
    return (wins[index - 1], wins.count)
}
func parent(_ el: AXUIElement) -> AXUIElement? {
    guard let p = attr(el, kAXParentAttribute), CFGetTypeID(p) == AXUIElementGetTypeID() else { return nil }
    return (p as! AXUIElement)
}

// MARK: - Menus, read through accessibility (never System Events).
func menuBar() -> AXUIElement? {
    if let m = attr(appEl, "AXMenuBar"), CFGetTypeID(m) == AXUIElementGetTypeID() { return (m as! AXUIElement) }
    return children(appEl).first { str($0, kAXRoleAttribute) == "AXMenuBar" }
}
/// The menu open right now: a menu bar menu (its bar item is selected) or a
/// context menu. An app in menu tracking may not answer every read; a failed
/// read is simply "not this one".
func openMenu() -> AXUIElement? {
    if let bar = menuBar() {
        for item in children(bar) where bool(item, kAXSelectedAttribute, false) {
            if let m = children(item).first(where: { str($0, kAXRoleAttribute) == "AXMenu" }) { return m }
        }
    }
    if let menus = attr(appEl, "AXMenus") as? [AXUIElement],
       let m = menus.first(where: { str($0, kAXRoleAttribute) == "AXMenu" && bool($0, "AXVisible", true) }) { return m }
    var queue: [(AXUIElement, Int)] = children(appEl).map { ($0, 0) }
    if let f = attr(appEl, kAXFocusedUIElementAttribute), CFGetTypeID(f) == AXUIElementGetTypeID() {
        queue.insert((f as! AXUIElement, 0), at: 0)
    }
    var seen = 0
    while !queue.isEmpty && seen < 300 {
        let (el, depth) = queue.removeFirst(); seen += 1
        let role = str(el, kAXRoleAttribute) ?? ""
        if role == "AXMenuBar" || role == "AXWindow" { continue }
        if role == "AXMenu" && bool(el, "AXVisible", true) { return el }
        if depth < 3 { queue += children(el).map { ($0, depth + 1) } }
    }
    // A context menu opens under the pointer, and some (SwiftUI's) are in no
    // list above: look just below and beside the pointer for a menu item of
    // this app and take its menu.
    if let at = CGEvent(source: nil)?.location {
        for (dx, dy) in [(14.0, 10.0), (14.0, 30.0), (-14.0, 10.0), (14.0, -10.0)] {
            var hit: AXUIElement?
            guard AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(), Float(at.x + dx), Float(at.y + dy), &hit) == .success,
                  let el = hit else { continue }
            var pid: pid_t = 0
            AXUIElementGetPid(el, &pid)
            guard pid == app.processIdentifier else { continue }
            var cur: AXUIElement? = el
            var hops = 0
            while let c = cur, hops < 6 {
                if str(c, kAXRoleAttribute) == "AXMenu" { return c }
                cur = parent(c); hops += 1
            }
        }
    }
    return nil
}
func menuNorm(_ s: String) -> String {
    s.replacingOccurrences(of: "...", with: "…").trimmingCharacters(in: .whitespaces).lowercased()
}
func menuItems(_ menu: AXUIElement) -> [AXUIElement] {
    children(menu).filter {
        let r = str($0, kAXRoleAttribute) ?? ""
        return (r == "AXMenuItem" || r == "AXMenuBarItem") && !identity($0).isEmpty
    }
}
func submenu(_ item: AXUIElement) -> AXUIElement? { children(item).first { str($0, kAXRoleAttribute) == "AXMenu" } }
func shortcut(_ item: AXUIElement) -> String? {
    guard let ch = str(item, "AXMenuItemCmdChar"), !ch.isEmpty else { return nil }
    let m = Int(str(item, "AXMenuItemCmdModifiers") ?? "0") ?? 0
    var s = ""
    if m & 4 != 0 { s += "⌃" }
    if m & 2 != 0 { s += "⌥" }
    if m & 1 != 0 { s += "⇧" }
    if m & 8 == 0 { s += "⌘" }
    return s + ch
}
/// One item of a menu by its title: exact (ignoring case and "..." vs "…"),
/// else the one item that starts with it. Names what is there when it fails.
func menuItem(_ menu: AXUIElement, _ name: String, _ where_: String) -> AXUIElement {
    let items = menuItems(menu)
    let want = menuNorm(name)
    if let hit = items.first(where: { menuNorm(identity($0)) == want }) { return hit }
    let prefixed = items.filter { menuNorm(identity($0)).hasPrefix(want) }
    if prefixed.count == 1 { return prefixed[0] }
    let names = items.map { identity($0) }.prefix(40).joined(separator: ", ")
    fail("no menu item \"\(name)\" in \(where_); it has: \(names)")
}
/// Walk "File > Export…" down the menu bar. Submenus some apps only fill in
/// when open are opened on the way.
func menuPath(_ path: String) -> [AXUIElement] {
    let parts = path.split(separator: ">").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    guard !parts.isEmpty else { fail("menu needs --path like \"File > Export…\"", 2) }
    guard let bar = menuBar() else { fail("\(app.localizedName ?? appSpec) has no menu bar readable through accessibility") }
    // One name that is not a menu title: the one item with that name in any
    // menu ("Say Hello" is Fixture > Say Hello).
    if parts.count == 1, !menuItems(bar).contains(where: { menuNorm(identity($0)) == menuNorm(parts[0]) }) {
        var hits: [[AXUIElement]] = []
        for top in menuItems(bar) {
            guard let sub = submenu(top) else { continue }
            for item in menuItems(sub) where menuNorm(identity(item)) == menuNorm(parts[0]) { hits.append([top, item]) }
        }
        if hits.count == 1 { return hits[0] }
        if hits.count > 1 {
            fail("\"\(parts[0])\" is in more than one menu: " + hits.map { "\(identity($0[0])) > \(identity($0[1]))" }.joined(separator: ", ") + "; name the menu too")
        }
    }
    var chain: [AXUIElement] = []
    var container = bar
    var where_ = "the menu bar"
    for (n, part) in parts.enumerated() {
        let item = menuItem(container, part, where_)
        chain.append(item)
        if n == parts.count - 1 { break }
        guard var sub = submenu(item) else { fail("\"\(part)\" has no submenu") }
        if menuItems(sub).isEmpty {
            AXUIElementPerformAction(item, kAXPressAction as CFString)
            usleep(150_000)
            sub = submenu(item) ?? sub
        }
        container = sub
        where_ = "\"\(part)\""
    }
    return chain
}
func waitFor(_ ms: Double, every: useconds_t = 25_000, _ cond: () -> Bool) -> Bool {
    let start = Date()
    repeat {
        if cond() { return true }
        usleep(every)
    } while Date().timeIntervalSince(start) * 1000 < ms
    return cond()
}

/// Every element under `root` that matches, within a node and time budget.
/// `stopAt` ends the search early once that many are found.
func findAll(_ root: AXUIElement, stopAt: Int = 2, nodes: Int = 1500, seconds: Double = 1.5,
             _ matches: (AXUIElement) -> Bool) -> [AXUIElement] {
    var found: [AXUIElement] = []
    var visited = 0
    let start = Date()
    func search(_ el: AXUIElement, _ depth: Int) {
        if found.count >= stopAt || visited > nodes || Date().timeIntervalSince(start) > seconds { return }
        visited += 1
        if matches(el) { found.append(el); if found.count >= stopAt { return } }
        if depth < 30 { for c in children(el) { search(c, depth + 1) } }
    }
    search(root, 0)
    return found
}

/// Visible through every clipping ancestor (window, scroll area, web area,
/// sheet, popover) — not just inside the window's rectangle.
let clippingRoles: Set<String> = ["AXWindow", "AXScrollArea", "AXWebArea", "AXSheet", "AXPopover"]
func visibleRect(_ el: AXUIElement) -> CGRect? {
    guard var r = frame(el), r.width > 0, r.height > 0 else { return nil }
    var cur = parent(el)
    var hops = 0
    while let c = cur, hops < 50 {
        let role = str(c, kAXRoleAttribute) ?? ""
        if role == "AXApplication" { break }
        if clippingRoles.contains(role), let f = frame(c), f.width > 0, f.height > 0 {
            r = r.intersection(f)
            if r.isNull || r.width < 1 || r.height < 1 { return nil }
        }
        cur = parent(c); hops += 1
    }
    return r
}
/// The text of `el` a person can see now. A text area's value is the whole
/// document (TextEdit: all 300 lines), so "is this text on screen" must ask
/// for the visible character range, not the value.
func visibleText(_ el: AXUIElement) -> String {
    guard visibleRect(el) != nil else { return "" }
    var parts: [String] = []
    if let t = str(el, kAXTitleAttribute) { parts.append(t) }
    if let d = describe(el) { parts.append(d) }
    let role = str(el, kAXRoleAttribute) ?? ""
    if role == "AXSecureTextField" { return parts.joined(separator: " ") }
    if let range = attr(el, "AXVisibleCharacterRange"), CFGetTypeID(range) == AXValueGetTypeID() {
        var out: CFTypeRef?
        if AXUIElementCopyParameterizedAttributeValue(el, "AXStringForRange" as CFString, range, &out) == .success,
           let text = out as? String {
            parts.append(text)
            return parts.joined(separator: " ")
        }
    }
    if let v = str(el, kAXValueAttribute) { parts.append(v) }
    return parts.joined(separator: " ")
}

func nearestScrollArea(_ el: AXUIElement) -> AXUIElement? {
    var cur = parent(el)
    var hops = 0
    while let c = cur, hops < 12 {
        if str(c, kAXRoleAttribute) == "AXScrollArea" { return c }
        cur = parent(c); hops += 1
    }
    return nil
}

func node(at path: String, in win: AXUIElement) -> AXUIElement? {
    var el = win
    for part in path.split(separator: ".") {
        guard let idx = Int(part) else { return nil }
        let kids = children(el)
        guard idx < kids.count else { return nil }
        el = kids[idx]
    }
    return el
}

/// What names an element across captures: its title, description or
/// placeholder. Never its value — a field's contents change when typed into,
/// and a secure field's contents are never read at all.
/// The description, or for a nameless element with a subrole its role
/// description: the window's traffic lights carry no title or description,
/// and "close button" is what keeps the model from pressing one blind
/// (Stadium, 2026-09-23: B23 "" closed Calculator).
func describe(_ el: AXUIElement) -> String? {
    if let d = str(el, kAXDescriptionAttribute), !d.isEmpty { return d }
    if let sub = str(el, kAXSubroleAttribute), !sub.isEmpty, sub != "AXUnknown",
       let rd = str(el, kAXRoleDescriptionAttribute), !rd.isEmpty { return rd }
    return nil
}
/// Roles a person types into: their value is contents, never their name.
let editableRoles: Set<String> = ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField", "AXSlider", "AXIncrementor", "AXDateField", "AXTimeField"]

/// The same name the tool lists the element by: title, description or
/// placeholder; for text that is not typed into (a static text, a label) its
/// value, which is what it says. A field's contents and a secure field never
/// name it. (Static text had no name here while the list named it by value,
/// so every right-click on a label was refused as stale — Stadium, 2026-09-24.)
func identity(_ el: AXUIElement) -> String {
    if let t = str(el, kAXTitleAttribute), !t.isEmpty { return t }
    if let d = describe(el) { return d }
    if let p = str(el, "AXPlaceholderValue"), !p.isEmpty { return p }
    let role = str(el, kAXRoleAttribute) ?? ""
    if role != "AXSecureTextField", !editableRoles.contains(role), let v = str(el, kAXValueAttribute) { return v }
    return ""
}

/// The element a recorded path points at, re-identified against the live
/// tree before anything acts on it. With `role` and `label`, the node at the
/// path must still be that element; otherwise the window is searched for the
/// one element with that role and label. Zero or several matches refuse: an
/// old observation never becomes a live mutation of the wrong thing.
func locate(_ path: String, in win: AXUIElement, role: String?, label: String?) -> AXUIElement {
    let atPath = node(at: path, in: win)
    guard let role = role, !role.isEmpty else {
        guard let el = atPath else { fail("no element at path \(path); the window changed — walk it again") }
        return el
    }
    let label = label ?? ""
    let matches: (AXUIElement) -> Bool = { el in
        (str(el, kAXRoleAttribute) ?? "") == role && (label.isEmpty || identity(el) == label)
    }
    if let el = atPath, matches(el) { return el }
    let found = findAll(win, stopAt: 2, matches)
    switch found.count {
    case 1: return found[0]
    case 0: fail("stale: no \(role) \"\(label)\" is on this window now; capture again")
    default: fail("ambiguous: \(found.count) elements are \(role) \"\(label)\"; capture again and use the one you mean")
    }
}

/// Bring the element's own window forward, not just its app: physical input
/// lands on whatever window is on top at the point.
func raise(_ win: AXUIElement) {
    app.activate(options: [])
    AXUIElementPerformAction(win, kAXRaiseAction as CFString)
    usleep(120_000)
}

let windowIndex = Int(params["window"] ?? "1") ?? 1

/// The element a path is relative to: the menu open now for "m:" paths,
/// otherwise the window.
func rootFor(_ path: String) -> (AXUIElement, String) {
    if path.hasPrefix("m:") {
        guard let m = openMenu() else { fail("stale: the menu is no longer open; open it again") }
        return (m, String(path.dropFirst(2)))
    }
    return (window(windowIndex).0, path)
}

/// The items of an open menu as tree nodes with "m:" paths, so they get refs
/// and are pressed like anything else.
@discardableResult
func emitMenuItems(_ m: AXUIElement) -> Int {
    var n = 0
    for (idx, item) in children(m).enumerated() {
        let role = str(item, kAXRoleAttribute) ?? ""
        let name = identity(item)
        guard role == "AXMenuItem", !name.isEmpty, let f = frame(item) else { continue }
        var node: [String: Any] = [
            "path": "m:\(idx)", "role": role, "title": name, "value": NSNull(), "placeholder": NSNull(), "desc": NSNull(),
            "frame": [Int(f.minX.rounded()), Int(f.minY.rounded()), Int(f.width.rounded()), Int(f.height.rounded())],
            "actions": bool(item, kAXEnabledAttribute, true) ? ["AXPress"] : [],
            "enabled": bool(item, kAXEnabledAttribute, true), "focused": false, "menu": true,
        ]
        if let k = shortcut(item) { node["shortcut"] = k }
        if submenu(item) != nil { node["submenu"] = true }
        emit(node)
        n += 1
    }
    return n
}

switch command {
case "tree":
    let maxDepth = Int(params["depth"] ?? "30") ?? 30
    let maxNodes = Int(params["max"] ?? "400") ?? 400
    let timeoutMs = Double(params["timeout-ms"] ?? "2000") ?? 2000
    let start = Date()
    let (win, winCount) = window(windowIndex)
    emit(["app": app.localizedName ?? appSpec, "pid": Int(app.processIdentifier), "windows": winCount])
    let winFrame = frame(win) ?? CGRect(x: 0, y: 0, width: 1e6, height: 1e6)
    let flattenRoles: Set<String> = ["AXGroup", "AXSplitGroup", "AXScrollArea", "AXLayoutArea"]
    let actionAllow: Set<String> = ["AXPress", "AXShowMenu", "AXIncrement", "AXDecrement", "AXConfirm", "AXCancel", "AXPick", "AXRaise"]
    var emitted = 0
    var truncated = false
    var cutBy = ""
    var resumeAt = ""

    // A menu open right now is its own surface, listed first: its items are
    // what the next click is for.
    if params["root"] == nil, let m = openMenu() { emitted += emitMenuItems(m) }

    func walk(_ el: AXUIElement, _ path: [String], _ depth: Int) {
        if truncated { return }
        if emitted >= maxNodes {
            truncated = true; cutBy = "node budget (\(maxNodes))"; resumeAt = path.joined(separator: "."); return
        }
        if Date().timeIntervalSince(start) * 1000 > timeoutMs {
            truncated = true; cutBy = "time budget (\(Int(timeoutMs)) ms)"; resumeAt = path.joined(separator: "."); return
        }
        let role = str(el, kAXRoleAttribute) ?? ""
        let f = frame(el)
        let onScreen = f.map { $0.width > 0 && $0.height > 0 && $0.intersects(winFrame) } ?? false
        let title = str(el, kAXTitleAttribute) ?? ""
        // Children the depth limit cuts off are counted on the node that
        // holds them, which is then listed even if it is a plain container,
        // so it can be drilled into.
        let more = depth >= maxDepth ? children(el).count : 0
        if onScreen && (!(flattenRoles.contains(role) && title.isEmpty) || more > 0) {
            var actions: [String] = []
            var names: CFArray?
            if AXUIElementCopyActionNames(el, &names) == .success, let list = names as? [String] {
                actions = list.filter { actionAllow.contains($0) }
            }
            // Chromium answers "settable" for every element; only roles a
            // person edits get AXSetValue.
            var settable = DarwinBoolean(false)
            if editableRoles.contains(role),
               AXUIElementIsAttributeSettable(el, kAXValueAttribute as CFString, &settable) == .success, settable.boolValue {
                actions.append("AXSetValue")
            }
            // A secure field's value is never read: it would land in the
            // transcript. Its placeholder still identifies it.
            let value = role == "AXSecureTextField" ? nil : str(el, kAXValueAttribute).map { String($0.prefix(200)) }
            let desc = describe(el)
            let placeholder = str(el, "AXPlaceholderValue")
            var node: [String: Any] = [
                "path": path.joined(separator: "."),
                "role": role,
                "title": title,
                "value": value ?? NSNull(),
                "placeholder": (placeholder?.isEmpty == false ? placeholder! : NSNull()),
                "desc": (desc?.isEmpty == false ? desc! : NSNull()),
                "frame": [Int(f!.minX.rounded()), Int(f!.minY.rounded()), Int(f!.width.rounded()), Int(f!.height.rounded())],
                "actions": actions,
                "enabled": bool(el, kAXEnabledAttribute, true),
                "focused": bool(el, kAXFocusedAttribute, false),
            ]
            if more > 0 { node["more"] = more }
            emit(node)
            emitted += 1
        }
        // A frame entirely off the window has no visible children; a missing
        // or zero frame (some containers) still can.
        if let f = f, f.width > 0, f.height > 0, !f.intersects(winFrame) { return }
        if depth < maxDepth {
            for (idx, child) in children(el).enumerated() { walk(child, path + [String(idx)], depth + 1) }
        }
    }
    // `--root` drills into one node: its subtree, with paths still from the window.
    if let root = params["root"], !root.isEmpty {
        guard let r = node(at: root, in: win) else { fail("stale: no element at \(root) now; capture again") }
        let base = root.split(separator: ".").map(String.init)
        for (idx, child) in children(r).enumerated() { walk(child, base + [String(idx)], 1) }
    } else {
        for (idx, child) in children(win).enumerated() { walk(child, [String(idx)], 1) }
    }
    var footer: [String: Any] = ["truncated": truncated, "elapsed_ms": Int(Date().timeIntervalSince(start) * 1000)]
    if truncated { footer["cut_by"] = cutBy; footer["resume_at"] = resumeAt }
    emit(footer)

case "window":
    // The window's frame and its CGWindowID, so it can be captured by id
    // (its own pixels, even under other windows) instead of by screen region.
    let (win, winCount) = window(windowIndex)
    guard let f = frame(win) else { fail("could not read the window frame") }
    var best: (Int, Double)? = nil
    if let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] {
        for w in list {
            guard (w[kCGWindowOwnerPID as String] as? Int32) == app.processIdentifier,
                  (w[kCGWindowLayer as String] as? Int) == 0,
                  let num = w[kCGWindowNumber as String] as? Int,
                  let b = w[kCGWindowBounds as String] as? [String: Double] else { continue }
            let d = abs((b["X"] ?? 0) - f.minX) + abs((b["Y"] ?? 0) - f.minY) + abs((b["Width"] ?? 0) - f.width) + abs((b["Height"] ?? 0) - f.height)
            if best == nil || d < best!.1 { best = (num, d) }
        }
    }
    var obj: [String: Any] = ["frame": [Int(f.minX.rounded()), Int(f.minY.rounded()), Int(f.width.rounded()), Int(f.height.rounded())],
                              "windows": winCount, "frontmost": app.isActive]
    if let b = best, b.1 < 8 { obj["window_id"] = b.0 }
    emit(obj)

case "act":
    guard let path = params["path"], let action = params["action"] else { fail("act needs --path and --action", 2) }
    let (root, rel) = rootFor(path)
    let el = locate(rel, in: root, role: params["role"], label: params["label"])
    if params["raise"] != nil, !path.hasPrefix("m:") { raise(root) }
    // A mutation gets longer than a read: a slow app must not turn a press
    // into "cannot complete" when it would have happened.
    AXUIElementSetMessagingTimeout(el, 2.0)
    let r = AXUIElementPerformAction(el, action as CFString)
    if r != .success { fail("\(action) on \(path) failed (AXError \(r.rawValue))") }

case "set":
    guard let path = params["path"], let value = params["value"] else { fail("set needs --path and --value", 2) }
    let (root, rel) = rootFor(path)
    let el = locate(rel, in: root, role: params["role"], label: params["label"])
    AXUIElementSetMessagingTimeout(el, 2.0)
    let r = AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, value as CFTypeRef)
    if r != .success { fail("set value on \(path) failed (AXError \(r.rawValue))") }
    // Delivered is not verified: read the field back.
    let now = str(el, kAXValueAttribute) ?? ""
    if now != value { fail("value was set but the field now reads \"\(now.prefix(80))\"") }

case "raise":
    let (win, _) = window(windowIndex)
    raise(win)

case "menu":
    guard let path = params["path"] else { fail("menu needs --path like \"File > Export…\"", 2) }
    let chain = menuPath(path)
    let leaf = chain.last!
    guard bool(leaf, kAXEnabledAttribute, true) else {
        fail("not delivered: \"\(identity(leaf))\" is disabled right now (greyed out in the menu)")
    }
    AXUIElementSetMessagingTimeout(leaf, 2.0)
    let r = AXUIElementPerformAction(leaf, kAXPressAction as CFString)
    if r != .success {
        AXUIElementPerformAction(chain[0], kAXCancelAction as CFString)
        fail("not delivered: pressing \"\(path)\" failed (AXError \(r.rawValue)); nothing was chosen")
    }
    let named = chain.map { identity($0) }.joined(separator: " > ")
    if chain.count == 1 {
        // A menu bar title opens its menu; its items are then listed as refs.
        let opened = waitFor(600) { openMenu() != nil }
        emit(["pressed": named, "opened": opened])
    } else {
        emit(["pressed": named, "menu_closed": waitFor(600) { openMenu() == nil }])
    }

case "menu-list":
    guard let bar = menuBar() else { fail("\(app.localizedName ?? appSpec) has no menu bar readable through accessibility") }
    var target = bar
    if let path = params["path"], !path.isEmpty {
        let chain = menuPath(path)
        guard let sub = submenu(chain.last!) else { fail("\"\(path)\" is a menu item, not a menu") }
        target = sub
    }
    for item in menuItems(target) {
        var o: [String: Any] = ["title": identity(item), "enabled": bool(item, kAXEnabledAttribute, true),
                                "submenu": submenu(item) != nil]
        if let k = shortcut(item) { o["shortcut"] = k }
        emit(o)
    }

case "show-menu":
    // A context menu through AXShowMenu on the element, then its ancestors;
    // proof is a menu that was not open before and is now.
    guard let path = params["path"] else { fail("show-menu needs --path", 2) }
    if openMenu() != nil { fail("not delivered: a menu is already open; choose from it or press escape first") }
    let (root, rel) = rootFor(path)
    let el = locate(rel, in: root, role: params["role"], label: params["label"])
    var target: AXUIElement? = el
    var tries = 0
    var opened = false
    while let t = target, tries < 4, !opened {
        AXUIElementSetMessagingTimeout(t, 2.0)
        if AXUIElementPerformAction(t, "AXShowMenu" as CFString) == .success {
            opened = waitFor(600) { openMenu() != nil }
        }
        target = parent(t); tries += 1
    }
    // Some toolkits (SwiftUI's .contextMenu) open their menu only for a real
    // right-click. Opening a menu changes nothing, so a right-click on the
    // element's own visible point is a safe second way — and it is checked
    // the same way: a menu that was not open is now.
    if !opened, let r = visibleRect(el) {
        let c = CGPoint(x: r.midX, y: r.midY)
        raise(root)
        for (type, button) in [(CGEventType.mouseMoved, CGMouseButton.right), (.rightMouseDown, .right), (.rightMouseUp, .right)] {
            CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: c, mouseButton: button)?.post(tap: .cghidEventTap)
            usleep(40_000)
        }
        opened = waitFor(900) { openMenu() != nil }
    }
    guard opened, let m = openMenu() else {
        fail("not delivered: no context menu opened (AXShowMenu on the element and its parents, then a right-click on it)")
    }
    emitMenuItems(m)

case "scroll-to":
    guard let path = params["path"] else { fail("scroll-to needs --path", 2) }
    let (root, rel) = rootFor(path)
    let el = locate(rel, in: root, role: params["role"], label: params["label"])
    if visibleRect(el) != nil { emit(["visible": true, "method": "already visible", "steps": 0]); exit(0) }
    AXUIElementSetMessagingTimeout(el, 2.0)
    if AXUIElementPerformAction(el, "AXScrollToVisible" as CFString) == .success,
       waitFor(800, every: 20_000, { visibleRect(el) != nil }) {
        emit(["visible": true, "method": "AXScrollToVisible", "steps": 1]); exit(0)
    }
    guard let area = nearestScrollArea(el), let af = frame(area) else {
        fail("not delivered: AXScrollToVisible did nothing and the element is not inside a scroll area")
    }
    var method = "scroll bar"
    for step in 1...10 {
        guard let f = frame(el) else { break }
        let vertical = f.midY > af.maxY || f.midY < af.minY || !(f.midX > af.maxX || f.midX < af.minX)
        let forward = vertical ? f.midY > af.midY : f.midX > af.midX
        var moved = false
        if let sbObj = attr(area, vertical ? "AXVerticalScrollBar" : "AXHorizontalScrollBar"),
           CFGetTypeID(sbObj) == AXUIElementGetTypeID() {
            let sb = sbObj as! AXUIElement
            if let v = attr(sb, kAXValueAttribute) as? NSNumber {
                let nv = max(0, min(1, v.doubleValue + (forward ? 0.12 : -0.12)))
                moved = AXUIElementSetAttributeValue(sb, kAXValueAttribute as CFString, NSNumber(value: nv)) == .success
            }
        }
        if !moved {
            // Last resort: the wheel over the scroll area's own center.
            method = "scroll wheel"
            let c = CGPoint(x: af.midX, y: af.midY)
            CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: c, mouseButton: .left)?.post(tap: .cghidEventTap)
            usleep(30_000)
            let amount = Int32(max(40, (vertical ? af.height : af.width) * 0.6)) * (forward ? -1 : 1)
            CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                    wheel1: vertical ? amount : 0, wheel2: vertical ? 0 : amount, wheel3: 0)?.post(tap: .cghidEventTap)
        }
        usleep(120_000)
        if visibleRect(el) != nil { emit(["visible": true, "method": method, "steps": step]); exit(0) }
    }
    fail("delivered, unverified: scrolled 10 steps by \(method) and the element is still not visible")

case "wait":
    let what = params["for"] ?? ""
    let timeout = min(Double(params["timeout-ms"] ?? "5000") ?? 5000, 60_000)
    let role = params["role"] ?? ""
    let label = (params["label"] ?? "").lowercased()
    let text = (params["text"] ?? "").lowercased()
    func firstWindow() -> AXUIElement? { windows().first }
    func labelled(_ el: AXUIElement) -> Bool {
        (role.isEmpty || str(el, kAXRoleAttribute) == role) && (label.isEmpty || identity(el).lowercased().contains(label))
    }
    // Only what is on screen counts: a text area's value is the whole document.
    func texted(_ el: AXUIElement) -> Bool { visibleText(el).lowercased().contains(text) }
    let baseline = windows().map { str($0, kAXTitleAttribute) ?? "" }
    var desc = ""
    var cond: () -> Bool
    switch what {
    case "menu": desc = "a menu to open"; cond = { openMenu() != nil }
    case "menu-closed": desc = "the menu to close"; cond = { openMenu() == nil }
    case "appears", "gone":
        guard !role.isEmpty || !label.isEmpty else { fail("wait --for \(what) needs --role or --label", 2) }
        desc = "\(role.isEmpty ? "an element" : role) \(label.isEmpty ? "" : "\"\(label)\" ")to \(what == "gone" ? "go away" : "appear")"
        let present = { () -> Bool in
            guard let w = firstWindow() else { return false }
            return !findAll(w, stopAt: 1, nodes: 3000, seconds: 0.8, labelled).isEmpty
        }
        cond = what == "gone" ? { !present() } : present
    case "text":
        guard !text.isEmpty else { fail("wait --for text needs --text", 2) }
        desc = "text \"\(text)\""
        cond = {
            guard let w = firstWindow() else { return false }
            return !findAll(w, stopAt: 1, nodes: 3000, seconds: 0.8, texted).isEmpty
        }
    case "value":
        guard let path = params["path"], let want = params["value"] else { fail("wait --for value needs --path and --value", 2) }
        desc = "the value to read \"\(want)\""
        cond = {
            guard let w = firstWindow(), let el = node(at: path, in: w) else { return false }
            return (str(el, kAXValueAttribute) ?? "") == want
        }
    case "window":
        let title = (params["title"] ?? "").lowercased()
        desc = title.isEmpty ? "a window to open, close or change title" : "a window titled \"\(title)\""
        cond = {
            let now = windows().map { str($0, kAXTitleAttribute) ?? "" }
            return title.isEmpty ? now != baseline : now.contains { $0.lowercased().contains(title) }
        }
    default:
        fail("wait --for must be appears, gone, text, value, menu, menu-closed or window", 2)
    }
    let started = Date()
    let slow = ["appears", "gone", "text"].contains(what)
    if waitFor(timeout, every: slow ? 150_000 : 75_000, cond) {
        emit(["found": true, "for": what, "elapsed_ms": Int(Date().timeIntervalSince(started) * 1000)])
    } else {
        fail("wait_timeout: waited \(Int(timeout)) ms for \(desc); it did not happen")
    }

default:
    fail("unknown command \(command); use tree, window, act, set, raise, menu, menu-list, show-menu, scroll-to, wait, text, frontmost, click, move, scroll, drag, key, type, hit", 2)
}
