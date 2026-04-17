// Nebo PIM Helper — native EventKit + Contacts replacement for AppleScript.
// Compile: swiftc -O -framework EventKit -framework Contacts -o pim-helper pim_helper.swift
// Usage:  pim-helper <domain> <action> [--key value ...]
//
// Matches OpenClaw's approach: EventKit for calendar/reminders, CNContactStore for contacts.
// Outputs pipe-separated text compatible with Nebo's existing ToolResult parsing.

import EventKit
import Contacts
import Foundation

// MARK: - Argument Parsing

let args = CommandLine.arguments
guard args.count >= 3 else {
    fputs("Usage: pim-helper <domain> <action> [--key value ...]\n", stderr)
    exit(1)
}
let domain = args[1]
let action = args[2]
var params: [String: String] = [:]
var i = 3
while i < args.count {
    if args[i].hasPrefix("--"), i + 1 < args.count {
        params[String(args[i].dropFirst(2))] = args[i + 1]
        i += 2
    } else { i += 1 }
}

// MARK: - Stores

let eventStore = EKEventStore()
let contactStore = CNContactStore()

// MARK: - Permission Helpers

func ensureCalendarAccess() {
    let sem = DispatchSemaphore(value: 0)
    var granted = false
    if #available(macOS 14.0, *) {
        eventStore.requestFullAccessToEvents { g, _ in granted = g; sem.signal() }
    } else {
        eventStore.requestAccess(to: .event) { g, _ in granted = g; sem.signal() }
    }
    sem.wait()
    guard granted else { print("ERROR: CALENDAR_PERMISSION_REQUIRED — grant Calendar access in System Settings > Privacy & Security > Calendars"); exit(1) }
}

func ensureReminderAccess() {
    let sem = DispatchSemaphore(value: 0)
    var granted = false
    if #available(macOS 14.0, *) {
        eventStore.requestFullAccessToReminders { g, _ in granted = g; sem.signal() }
    } else {
        eventStore.requestAccess(to: .reminder) { g, _ in granted = g; sem.signal() }
    }
    sem.wait()
    guard granted else { print("ERROR: REMINDERS_PERMISSION_REQUIRED — grant Reminders access in System Settings > Privacy & Security > Reminders"); exit(1) }
}

func ensureContactsAccess() {
    let sem = DispatchSemaphore(value: 0)
    var granted = false
    contactStore.requestAccess(for: .contacts) { g, _ in granted = g; sem.signal() }
    sem.wait()
    guard granted else { print("ERROR: CONTACTS_PERMISSION_REQUIRED — grant Contacts access in System Settings > Privacy & Security > Contacts"); exit(1) }
}

// MARK: - Date Parsing

let isoFormatter: ISO8601DateFormatter = {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    return f
}()

let displayFormatter: DateFormatter = {
    let f = DateFormatter()
    f.dateStyle = .medium
    f.timeStyle = .short
    return f
}()

func parseDate(_ s: String) -> Date? {
    // ISO 8601
    if let d = isoFormatter.date(from: s) { return d }
    // yyyy-MM-dd HH:mm
    let df = DateFormatter()
    df.dateFormat = "yyyy-MM-dd HH:mm"
    if let d = df.date(from: s) { return d }
    // yyyy-MM-dd HH:mm:ss
    df.dateFormat = "yyyy-MM-dd HH:mm:ss"
    if let d = df.date(from: s) { return d }
    // yyyy-MM-dd
    df.dateFormat = "yyyy-MM-dd"
    if let d = df.date(from: s) { return d }
    // AppleScript format: "January 2, 2006 at 3:04:05 PM"
    df.dateFormat = "MMMM d, yyyy 'at' h:mm:ss a"
    if let d = df.date(from: s) { return d }
    // Natural language
    let lower = s.lowercased().trimmingCharacters(in: .whitespaces)
    let cal = Calendar.current
    let now = Date()
    if lower == "today" { return cal.startOfDay(for: now) }
    if lower == "tomorrow" { return cal.date(byAdding: .day, value: 1, to: cal.startOfDay(for: now)) }
    // "in N hours/days/weeks"
    let inPattern = try? NSRegularExpression(pattern: #"^in\s+(\d+)\s+(hour|day|week|minute)s?$"#)
    if let match = inPattern?.firstMatch(in: lower, range: NSRange(lower.startIndex..., in: lower)),
       let numRange = Range(match.range(at: 1), in: lower),
       let unitRange = Range(match.range(at: 2), in: lower),
       let num = Int(lower[numRange]) {
        let unit = String(lower[unitRange])
        switch unit {
        case "minute": return cal.date(byAdding: .minute, value: num, to: now)
        case "hour": return cal.date(byAdding: .hour, value: num, to: now)
        case "day": return cal.date(byAdding: .day, value: num, to: now)
        case "week": return cal.date(byAdding: .day, value: num * 7, to: now)
        default: break
        }
    }
    return nil
}

/// Format a Date for AppleScript's `date "..."` literal.
func appleScriptDate(_ d: Date) -> String {
    let f = DateFormatter()
    f.dateFormat = "MMMM d, yyyy 'at' h:mm:ss a"
    return f.string(from: d)
}

// MARK: - Calendar

func calendarList() {
    ensureCalendarAccess()
    let names = eventStore.calendars(for: .event).map { $0.title }
    print(names.joined(separator: ", "))
}

func calendarEvents() {
    ensureCalendarAccess()
    let days = Int(params["days"] ?? "1") ?? 1
    let calName = params["calendar"]

    let cal = Calendar.current
    let start = cal.startOfDay(for: Date())
    guard let end = cal.date(byAdding: .day, value: days, to: start) else {
        print("ERROR: Invalid date range"); exit(1)
    }

    var calendars: [EKCalendar]? = nil
    if let name = calName, !name.isEmpty {
        calendars = eventStore.calendars(for: .event).filter {
            $0.title.compare(name, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }
        if calendars?.isEmpty == true {
            print("ERROR: Calendar '\(name)' not found"); exit(1)
        }
    }

    let pred = eventStore.predicateForEvents(withStart: start, end: end, calendars: calendars)
    let events = eventStore.events(matching: pred).sorted { $0.startDate < $1.startDate }

    if events.isEmpty {
        print(days <= 1 ? "No events today" : "No upcoming events in the next \(days) days")
        return
    }

    for e in events {
        print("\(e.calendar.title) | \(e.title ?? "Untitled") | \(displayFormatter.string(from: e.startDate))")
    }
}

func calendarCreate() {
    ensureCalendarAccess()
    let title = params["title"] ?? params["name"] ?? ""
    guard !title.isEmpty else { print("ERROR: 'title' or 'name' required"); exit(1) }
    let dateStr = params["date"] ?? params["start"] ?? ""
    guard !dateStr.isEmpty else { print("ERROR: 'date' required (e.g. '2025-06-15 14:00')"); exit(1) }
    guard let startDate = parseDate(dateStr) else { print("ERROR: Invalid date '\(dateStr)'"); exit(1) }

    let endDate: Date
    if let endStr = params["end_date"] ?? params["end"], !endStr.isEmpty {
        guard let d = parseDate(endStr) else { print("ERROR: Invalid end_date"); exit(1) }
        endDate = d
    } else {
        endDate = startDate.addingTimeInterval(3600)
    }

    let event = EKEvent(eventStore: eventStore)
    event.title = title
    event.startDate = startDate
    event.endDate = endDate
    if let loc = params["location"], !loc.isEmpty { event.location = loc }
    if let notes = params["notes"], !notes.isEmpty { event.notes = notes }

    if let calName = params["calendar"], !calName.isEmpty {
        if let c = eventStore.calendars(for: .event).first(where: {
            $0.title.compare(calName, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }) {
            event.calendar = c
        } else {
            print("ERROR: Calendar '\(calName)' not found"); exit(1)
        }
    } else {
        event.calendar = eventStore.defaultCalendarForNewEvents
    }

    // Recurrence: --repeat daily|weekly|monthly|yearly [--interval N] [--end_repeat yyyy-MM-dd] [--days mon,tue,wed,...]
    if let repeatStr = params["repeat"], !repeatStr.isEmpty {
        let rule = parseRecurrence(repeatStr)
        event.addRecurrenceRule(rule)
    }

    do {
        try eventStore.save(event, span: .thisEvent)
        var msg = "Event created: \(title)"
        if event.hasRecurrenceRules { msg += " (recurring)" }
        print(msg)
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

func parseRecurrence(_ repeatStr: String) -> EKRecurrenceRule {
    let lower = repeatStr.lowercased().trimmingCharacters(in: .whitespaces)
    let interval = Int(params["interval"] ?? "1") ?? 1

    let freq: EKRecurrenceFrequency
    switch lower {
    case "daily": freq = .daily
    case "weekly": freq = .weekly
    case "monthly": freq = .monthly
    case "yearly": freq = .yearly
    default: freq = .daily
    }

    // Parse --days for weekly recurrence (e.g. "mon,tue,wed")
    var daysOfWeek: [EKRecurrenceDayOfWeek]? = nil
    if let daysStr = params["days"], !daysStr.isEmpty {
        let dayMap: [String: EKWeekday] = [
            "sun": .sunday, "mon": .monday, "tue": .tuesday,
            "wed": .wednesday, "thu": .thursday, "fri": .friday,
            "sat": .saturday,
            "sunday": .sunday, "monday": .monday, "tuesday": .tuesday,
            "wednesday": .wednesday, "thursday": .thursday, "friday": .friday,
            "saturday": .saturday,
        ]
        daysOfWeek = daysStr.lowercased().split(separator: ",")
            .compactMap { dayMap[String($0).trimmingCharacters(in: .whitespaces)] }
            .map { EKRecurrenceDayOfWeek($0) }
    }

    // Parse --end_repeat for recurrence end date
    var end: EKRecurrenceEnd? = nil
    if let endStr = params["end_repeat"], !endStr.isEmpty {
        if let endDate = parseDate(endStr) {
            end = EKRecurrenceEnd(end: endDate)
        }
    }

    return EKRecurrenceRule(
        recurrenceWith: freq,
        interval: interval,
        daysOfTheWeek: daysOfWeek,
        daysOfTheMonth: nil,
        monthsOfTheYear: nil,
        weeksOfTheYear: nil,
        daysOfTheYear: nil,
        setPositions: nil,
        end: end
    )
}

func calendarDelete() {
    ensureCalendarAccess()
    let title = params["title"] ?? params["name"] ?? ""
    guard !title.isEmpty else { print("ERROR: 'title' or 'name' required for delete"); exit(1) }

    let calName = params["calendar"] ?? ""
    let now = Date()
    let oneYearAhead = Calendar.current.date(byAdding: .year, value: 1, to: now)!
    let predicate = eventStore.predicateForEvents(withStart: now.addingTimeInterval(-86400 * 365), end: oneYearAhead, calendars: nil)
    let events = eventStore.events(matching: predicate)

    // Find matching events (case-insensitive title match, optional calendar filter)
    let matches = events.filter { ev in
        let titleMatch = ev.title.compare(title, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        if !calName.isEmpty {
            return titleMatch && ev.calendar.title.compare(calName, options: [.caseInsensitive]) == .orderedSame
        }
        return titleMatch
    }

    if matches.isEmpty {
        print("No events found matching '\(title)'")
        return
    }

    var deleted = 0
    for event in matches {
        do {
            // Use .futureEvents to delete recurring instances from now on
            let span: EKSpan = event.hasRecurrenceRules ? .futureEvents : .thisEvent
            try eventStore.remove(event, span: span, commit: true)
            deleted += 1
        } catch {
            print("ERROR: Failed to delete '\(event.title ?? "")': \(error.localizedDescription)")
        }
    }
    print("Deleted \(deleted) event(s) matching '\(title)'")
}

func calendarPending() {
    ensureCalendarAccess()
    let now = Date()
    let sixMonths = Calendar.current.date(byAdding: .month, value: 6, to: now)!
    let predicate = eventStore.predicateForEvents(withStart: now, end: sixMonths, calendars: nil)
    let events = eventStore.events(matching: predicate)

    let pending = events.filter { ev in
        guard let attendees = ev.attendees else { return false }
        return attendees.contains { att in
            att.isCurrentUser && att.participantStatus == .pending
        }
    }

    if pending.isEmpty {
        print("No pending calendar invitations")
        return
    }

    for ev in pending {
        let dateStr = displayFormatter.string(from: ev.startDate)
        let organizer = ev.organizer?.name ?? "unknown"
        let cal = ev.calendar.title
        print("\(ev.title ?? "(no title)") | \(dateStr) | from: \(organizer) | calendar: \(cal)")
    }
    print("\n\(pending.count) pending invitation(s)")
}

func calendarAccept() {
    ensureCalendarAccess()
    let title = params["title"] ?? params["name"] ?? ""
    let now = Date()
    let sixMonths = Calendar.current.date(byAdding: .month, value: 6, to: now)!
    let predicate = eventStore.predicateForEvents(withStart: now, end: sixMonths, calendars: nil)
    let events = eventStore.events(matching: predicate)

    // Find pending invitations — optionally filter by title
    let pending = events.filter { ev in
        guard let attendees = ev.attendees else { return false }
        let isPending = attendees.contains { att in
            att.isCurrentUser && att.participantStatus == .pending
        }
        if !isPending { return false }
        if !title.isEmpty {
            return ev.title?.compare(title, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }
        return true
    }

    if pending.isEmpty {
        print(title.isEmpty ? "No pending invitations to accept" : "No pending invitation matching '\(title)'")
        return
    }

    // EventKit's participantStatus is read-only. Use Calendar.app AppleScript
    // to accept invitations, which triggers the proper CalDAV/Exchange response.
    var accepted = 0
    for ev in pending {
        let eventTitle = ev.title ?? ""
        let calTitle = ev.calendar.title
        // AppleScript: find the event in Calendar.app and set its status
        let script = """
        tell application "Calendar"
            tell calendar "\(calTitle)"
                set matchingEvents to (every event whose summary is "\(eventTitle)" and start date is (date "\(appleScriptDate(ev.startDate))"))
                repeat with e in matchingEvents
                    set status of e to confirmed
                end repeat
            end tell
        end tell
        """
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        task.arguments = ["-e", script]
        let pipe = Pipe()
        task.standardOutput = pipe
        task.standardError = pipe
        do {
            try task.run()
            task.waitUntilExit()
            if task.terminationStatus == 0 {
                accepted += 1
                print("Accepted: \(eventTitle)")
            } else {
                let errData = pipe.fileHandleForReading.readDataToEndOfFile()
                let errStr = String(data: errData, encoding: .utf8) ?? ""
                print("Could not accept '\(eventTitle)': \(errStr.trimmingCharacters(in: .whitespacesAndNewlines))")
            }
        } catch {
            print("Could not accept '\(eventTitle)': \(error.localizedDescription)")
        }
    }
    if accepted > 0 {
        print("\nAccepted \(accepted) of \(pending.count) invitation(s)")
    } else {
        print("\nCould not accept invitations programmatically. Open Calendar.app to accept them manually.")
    }
}

func calendarDecline() {
    ensureCalendarAccess()
    let title = params["title"] ?? params["name"] ?? ""
    guard !title.isEmpty else { print("ERROR: 'title' or 'name' required to decline a specific invite"); exit(1) }
    let now = Date()
    let sixMonths = Calendar.current.date(byAdding: .month, value: 6, to: now)!
    let predicate = eventStore.predicateForEvents(withStart: now, end: sixMonths, calendars: nil)
    let events = eventStore.events(matching: predicate)

    let pending = events.filter { ev in
        guard let attendees = ev.attendees else { return false }
        let isPending = attendees.contains { att in
            att.isCurrentUser && (att.participantStatus == .pending || att.participantStatus == .accepted)
        }
        return isPending && ev.title?.compare(title, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
    }

    if pending.isEmpty {
        print("No invitation matching '\(title)'")
        return
    }

    var declined = 0
    for ev in pending {
        do {
            try eventStore.remove(ev, span: .thisEvent, commit: true)
            declined += 1
            print("Declined: \(ev.title ?? "(no title)")")
        } catch {
            print("Failed to decline '\(ev.title ?? "")': \(error.localizedDescription)")
        }
    }
    print("\nDeclined \(declined) invitation(s)")
}

// MARK: - Contacts

let contactKeys: [CNKeyDescriptor] = [
    CNContactIdentifierKey as CNKeyDescriptor,
    CNContactGivenNameKey as CNKeyDescriptor,
    CNContactFamilyNameKey as CNKeyDescriptor,
    CNContactOrganizationNameKey as CNKeyDescriptor,
    CNContactPhoneNumbersKey as CNKeyDescriptor,
    CNContactEmailAddressesKey as CNKeyDescriptor,
    CNContactNoteKey as CNKeyDescriptor,
]

func contactsSearch() {
    ensureContactsAccess()
    let query = params["query"] ?? ""
    guard !query.isEmpty else { print("ERROR: 'query' required"); exit(1) }
    let limit = Int(params["limit"] ?? "25") ?? 25

    do {
        let predicate = CNContact.predicateForContacts(matchingName: query)
        let contacts = try contactStore.unifiedContacts(matching: predicate, keysToFetch: contactKeys)
        let sliced = Array(contacts.prefix(limit))

        if sliced.isEmpty { print("No contacts found"); return }
        for c in sliced {
            let name = "\(c.givenName) \(c.familyName)".trimmingCharacters(in: .whitespaces)
            let email = c.emailAddresses.first.map { String($0.value) } ?? ""
            if email.isEmpty {
                print(name)
            } else {
                print("\(name) | \(email)")
            }
        }
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

func contactsGet() {
    ensureContactsAccess()
    let name = params["name"] ?? ""
    guard !name.isEmpty else { print("ERROR: 'name' required"); exit(1) }

    do {
        let predicate = CNContact.predicateForContacts(matchingName: name)
        let contacts = try contactStore.unifiedContacts(matching: predicate, keysToFetch: contactKeys)
        guard let c = contacts.first else { print("No contacts found"); return }

        let fullName = "\(c.givenName) \(c.familyName)".trimmingCharacters(in: .whitespaces)
        print("Name: \(fullName)")
        for email in c.emailAddresses {
            print("Email (\(email.label ?? "other")): \(email.value)")
        }
        for phone in c.phoneNumbers {
            print("Phone (\(phone.label ?? "other")): \(phone.value.stringValue)")
        }
        if !c.organizationName.isEmpty { print("Company: \(c.organizationName)") }
        if !c.note.isEmpty { print("Notes: \(c.note)") }
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

func contactsCreate() {
    ensureContactsAccess()
    let name = params["name"] ?? ""
    guard !name.isEmpty else { print("ERROR: 'name' required"); exit(1) }

    let parts = name.split(separator: " ", maxSplits: 1)
    let contact = CNMutableContact()
    contact.givenName = String(parts[0])
    if parts.count > 1 { contact.familyName = String(parts[1]) }

    if let email = params["email"], !email.isEmpty {
        contact.emailAddresses = [CNLabeledValue(label: CNLabelWork, value: email as NSString)]
    }
    if let phone = params["phone"], !phone.isEmpty {
        contact.phoneNumbers = [CNLabeledValue(label: CNLabelPhoneNumberMobile, value: CNPhoneNumber(stringValue: phone))]
    }
    if let company = params["company"], !company.isEmpty {
        contact.organizationName = company
    }
    if let notes = params["notes"], !notes.isEmpty {
        contact.note = notes
    }

    do {
        let req = CNSaveRequest()
        req.add(contact, toContainerWithIdentifier: nil)
        try contactStore.execute(req)
        print("Contact created: \(name)")
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

func contactsGroups() {
    ensureContactsAccess()
    do {
        let groups = try contactStore.groups(matching: nil)
        if groups.isEmpty { print("No groups"); return }
        print(groups.map { $0.name }.joined(separator: ", "))
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

// MARK: - Reminders

func reminderLists() {
    ensureReminderAccess()
    let cals = eventStore.calendars(for: .reminder)
    print(cals.map { $0.title }.joined(separator: ", "))
}

func reminderList() {
    ensureReminderAccess()
    let listName = params["list"]

    var calendars: [EKCalendar]? = nil
    if let name = listName, !name.isEmpty {
        calendars = eventStore.calendars(for: .reminder).filter {
            $0.title.compare(name, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }
    }

    let pred = eventStore.predicateForReminders(in: calendars)
    let sem = DispatchSemaphore(value: 0)
    var items: [EKReminder] = []
    eventStore.fetchReminders(matching: pred) { result in
        items = (result ?? []).filter { !$0.isCompleted }
        sem.signal()
    }
    sem.wait()

    if items.isEmpty {
        let listLabel = listName ?? "default"
        print("No reminders in list '\(listLabel)'")
        return
    }

    for r in items {
        var line = r.title ?? "Untitled"
        if let comps = r.dueDateComponents, let d = Calendar.current.date(from: comps) {
            line += " | Due: \(displayFormatter.string(from: d))"
        }
        if r.priority > 0 { line += " | Priority: \(r.priority)" }
        print(line)
    }
}

func reminderCreate() {
    ensureReminderAccess()
    let title = params["name"] ?? params["title"] ?? ""
    guard !title.isEmpty else { print("ERROR: 'name' required"); exit(1) }

    let reminder = EKReminder(eventStore: eventStore)
    reminder.title = title
    if let notes = params["notes"], !notes.isEmpty { reminder.notes = notes }

    // Priority: 1-4 = high (1), 5 = medium (5), 6-9 = low (9)
    if let priStr = params["priority"], let pri = Int(priStr) {
        switch pri {
        case 1...4: reminder.priority = 1
        case 5: reminder.priority = 5
        default: reminder.priority = 9
        }
    }

    // Resolve list
    let listName = params["list"]
    if let name = listName, !name.isEmpty {
        if let cal = eventStore.calendars(for: .reminder).first(where: {
            $0.title.compare(name, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }) {
            reminder.calendar = cal
        } else {
            print("ERROR: Reminder list '\(name)' not found"); exit(1)
        }
    } else {
        reminder.calendar = eventStore.defaultCalendarForNewReminders()
    }

    // Due date
    if let dueStr = params["due_date"], !dueStr.isEmpty {
        guard let d = parseDate(dueStr) else { print("ERROR: Invalid due_date '\(dueStr)'"); exit(1) }
        reminder.dueDateComponents = Calendar.current.dateComponents([.year, .month, .day, .hour, .minute, .second], from: d)
    }

    do {
        try eventStore.save(reminder, commit: true)
        print("Reminder created: \(title)")
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

func reminderComplete() {
    ensureReminderAccess()
    let title = params["name"] ?? params["title"] ?? ""
    guard !title.isEmpty else { print("ERROR: 'name' required"); exit(1) }

    let listName = params["list"]
    var calendars: [EKCalendar]? = nil
    if let name = listName, !name.isEmpty {
        calendars = eventStore.calendars(for: .reminder).filter {
            $0.title.compare(name, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }
    }

    let pred = eventStore.predicateForReminders(in: calendars)
    let sem = DispatchSemaphore(value: 0)
    var items: [EKReminder] = []
    eventStore.fetchReminders(matching: pred) { result in items = result ?? []; sem.signal() }
    sem.wait()

    guard let r = items.first(where: {
        ($0.title ?? "").compare(title, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame && !$0.isCompleted
    }) else { print("ERROR: Reminder '\(title)' not found"); exit(1) }

    r.isCompleted = true
    do {
        try eventStore.save(r, commit: true)
        print("Completed: \(title)")
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

func reminderDelete() {
    ensureReminderAccess()
    let title = params["name"] ?? params["title"] ?? ""
    guard !title.isEmpty else { print("ERROR: 'name' required"); exit(1) }

    let listName = params["list"]
    var calendars: [EKCalendar]? = nil
    if let name = listName, !name.isEmpty {
        calendars = eventStore.calendars(for: .reminder).filter {
            $0.title.compare(name, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
        }
    }

    let pred = eventStore.predicateForReminders(in: calendars)
    let sem = DispatchSemaphore(value: 0)
    var items: [EKReminder] = []
    eventStore.fetchReminders(matching: pred) { result in items = result ?? []; sem.signal() }
    sem.wait()

    guard let r = items.first(where: {
        ($0.title ?? "").compare(title, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
    }) else { print("ERROR: Reminder '\(title)' not found"); exit(1) }

    do {
        try eventStore.remove(r, commit: true)
        print("Deleted reminder: \(title)")
    } catch {
        print("ERROR: \(error.localizedDescription)"); exit(1)
    }
}

// MARK: - Dispatch

switch domain {
case "calendar":
    switch action {
    case "calendars": calendarList()
    case "events", "today", "upcoming", "list": calendarEvents()
    case "create": calendarCreate()
    case "delete": calendarDelete()
    case "pending": calendarPending()
    case "accept": calendarAccept()
    case "decline": calendarDecline()
    default: print("ERROR: Unknown calendar action '\(action)'"); exit(1)
    }
case "contacts":
    switch action {
    case "search": contactsSearch()
    case "get": contactsGet()
    case "create": contactsCreate()
    case "groups": contactsGroups()
    default: print("ERROR: Unknown contacts action '\(action)'"); exit(1)
    }
case "reminders":
    switch action {
    case "lists": reminderLists()
    case "list": reminderList()
    case "create": reminderCreate()
    case "complete": reminderComplete()
    case "delete": reminderDelete()
    default: print("ERROR: Unknown reminders action '\(action)'"); exit(1)
    }
default:
    print("ERROR: Unknown domain '\(domain)'"); exit(1)
}
