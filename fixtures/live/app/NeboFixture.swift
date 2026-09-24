// Nebo's computer-control fixture app: every hard case of desktop control in
// one window, deterministic, so live tests do not depend on what Calculator
// or TextEdit look like this year.
//
//   swiftc -O -parse-as-library NeboFixture.swift -o /tmp/nebo-fixture && /tmp/nebo-fixture &
//
// What it holds, and what each part tests:
//   Shuffle          reorders the three buttons below it (refs must follow labels, not positions)
//   Alpha/Beta/Gamma each sets Status to its name
//   Name             a text field with a placeholder (its contents never name it)
//   Password         a secure field (never read, never listed)
//   Delayed          sets Status to "Ready" 1.5 s after the press (wait_for)
//   Rows             200 rows; "Row 150" is off screen until scrolled (scroll until / scroll-to)
//   Tag              a label with a context menu: Copy Tag, Flag (right-click menus)
//   Fixture menu     Say Hello (menu bar)
// Status shows the last thing that happened, as text a capture can read.

import SwiftUI
import AppKit

@main
struct NeboFixtureApp: App {
    @StateObject private var model = FixtureModel()

    init() {
        // A bare executable is a background process until told otherwise.
        NSApplication.shared.setActivationPolicy(.regular)
        DispatchQueue.main.async { NSApplication.shared.activate(ignoringOtherApps: true) }
    }

    var body: some Scene {
        WindowGroup("Nebo Fixture") {
            FixtureView().environmentObject(model)
        }
        .commands {
            CommandMenu("Fixture") {
                Button("Say Hello") { model.status = "Hello from the menu" }
                    .keyboardShortcut("h", modifiers: [.command, .shift])
            }
        }
    }
}

final class FixtureModel: ObservableObject {
    @Published var status = "Idle"
    @Published var order = ["Alpha", "Beta", "Gamma"]
    @Published var name = ""
    @Published var password = ""
}

struct FixtureView: View {
    @EnvironmentObject var model: FixtureModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Status: \(model.status)").accessibilityIdentifier("status")
            HStack {
                Button("Shuffle") {
                    model.order = [model.order[2], model.order[0], model.order[1]]
                    model.status = "Shuffled"
                }
                ForEach(model.order, id: \.self) { name in
                    Button(name) { model.status = name }
                }
            }
            HStack {
                TextField("Your name", text: $model.name).frame(width: 180)
                SecureField("Password", text: $model.password).frame(width: 140)
                Button("Delayed") {
                    model.status = "Waiting…"
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { model.status = "Ready" }
                }
            }
            Text("Tag: nebo-42")
                .padding(4)
                .contextMenu {
                    Button("Copy Tag") { model.status = "Copied nebo-42" }
                    Button("Flag") { model.status = "Flagged" }
                }
            List(1...200, id: \.self) { n in
                Button("Row \(n)") { model.status = "Picked Row \(n)" }.buttonStyle(.plain)
            }
            .frame(height: 220)
        }
        .padding(14)
        .frame(width: 520, height: 420)
    }
}
