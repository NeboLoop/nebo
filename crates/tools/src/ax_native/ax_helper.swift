// Nebo accessibility helper — walks an app window's AXUIElement tree.
// Compile: swiftc -O -framework ApplicationServices -framework AppKit -o ax-helper ax_helper.swift
//
//   ax-helper tree --app <name|bundle id|pid> [--window 1] [--depth 30] [--max 400] [--timeout-ms 2000]
//   ax-helper act  --app … [--window 1] --path 0.3.2 --action AXPress
//   ax-helper set  --app … [--window 1] --path 0.3.2 --value "text"
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
func identity(_ el: AXUIElement) -> String {
    if let t = str(el, kAXTitleAttribute), !t.isEmpty { return t }
    if let d = str(el, kAXDescriptionAttribute), !d.isEmpty { return d }
    if let p = str(el, "AXPlaceholderValue"), !p.isEmpty { return p }
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
    var found: [AXUIElement] = []
    var visited = 0
    let start = Date()
    func search(_ el: AXUIElement, _ depth: Int) {
        if found.count > 1 || visited > 1500 || Date().timeIntervalSince(start) > 1.5 { return }
        visited += 1
        if matches(el) { found.append(el); return }
        if depth < 30 { for c in children(el) { search(c, depth + 1) } }
    }
    search(win, 0)
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
    let editableRoles: Set<String> = ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField", "AXSlider", "AXIncrementor", "AXDateField", "AXTimeField"]
    let actionAllow: Set<String> = ["AXPress", "AXShowMenu", "AXIncrement", "AXDecrement", "AXConfirm", "AXCancel", "AXPick", "AXRaise"]
    var emitted = 0
    var truncated = false

    func walk(_ el: AXUIElement, _ path: [Int], _ depth: Int) {
        if truncated { return }
        if emitted >= maxNodes || Date().timeIntervalSince(start) * 1000 > timeoutMs { truncated = true; return }
        let role = str(el, kAXRoleAttribute) ?? ""
        let f = frame(el)
        let onScreen = f.map { $0.width > 0 && $0.height > 0 && $0.intersects(winFrame) } ?? false
        let title = str(el, kAXTitleAttribute) ?? ""
        if onScreen && !(flattenRoles.contains(role) && title.isEmpty) {
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
            let desc = str(el, kAXDescriptionAttribute)
            let placeholder = str(el, "AXPlaceholderValue")
            emit([
                "path": path.map(String.init).joined(separator: "."),
                "role": role,
                "title": title,
                "value": value ?? NSNull(),
                "placeholder": (placeholder?.isEmpty == false ? placeholder! : NSNull()),
                "desc": (desc?.isEmpty == false ? desc! : NSNull()),
                "frame": [Int(f!.minX.rounded()), Int(f!.minY.rounded()), Int(f!.width.rounded()), Int(f!.height.rounded())],
                "actions": actions,
                "enabled": bool(el, kAXEnabledAttribute, true),
                "focused": bool(el, kAXFocusedAttribute, false),
            ])
            emitted += 1
        }
        // A frame entirely off the window has no visible children; a missing
        // or zero frame (some containers) still can.
        if let f = f, f.width > 0, f.height > 0, !f.intersects(winFrame) { return }
        if depth < maxDepth {
            for (idx, child) in children(el).enumerated() { walk(child, path + [idx], depth + 1) }
        }
    }
    for (idx, child) in children(win).enumerated() { walk(child, [idx], 1) }
    emit(["truncated": truncated, "elapsed_ms": Int(Date().timeIntervalSince(start) * 1000)])

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
    let (win, _) = window(windowIndex)
    let el = locate(path, in: win, role: params["role"], label: params["label"])
    if params["raise"] != nil { raise(win) }
    let r = AXUIElementPerformAction(el, action as CFString)
    if r != .success { fail("\(action) on \(path) failed (AXError \(r.rawValue))") }

case "set":
    guard let path = params["path"], let value = params["value"] else { fail("set needs --path and --value", 2) }
    let (win, _) = window(windowIndex)
    let el = locate(path, in: win, role: params["role"], label: params["label"])
    let r = AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, value as CFTypeRef)
    if r != .success { fail("set value on \(path) failed (AXError \(r.rawValue))") }
    // Delivered is not verified: read the field back.
    let now = str(el, kAXValueAttribute) ?? ""
    if now != value { fail("value was set but the field now reads \"\(now.prefix(80))\"") }

case "raise":
    let (win, _) = window(windowIndex)
    raise(win)

default:
    fail("unknown command \(command); use tree, window, act, set, raise, text", 2)
}
