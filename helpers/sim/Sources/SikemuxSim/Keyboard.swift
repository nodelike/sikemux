import FBSimulatorControl
import Foundation

/// USB HID usage codes for the characters a US keyboard types, and whether each needs Shift.
enum Keyboard {
    static let shift: UInt32 = 225
    /// Brief hold between key down and up (and around Shift) so dtuhidd/backboardd do not coalesce zero-duration keypresses.
    static let hold: TimeInterval = 0.015
    /// Pause between keys so iOS Remote Text Input and JS-controlled inputs do not drop fast sequences.
    static let pause: TimeInterval = 0.045

    static func press(_ code: UInt32) -> SimulatorHIDEvent {
        .composite([
            .keyboard(direction: .down, keyCode: code),
            .delay(hold),
            .keyboard(direction: .up, keyCode: code),
        ])
    }

    static func events(for text: String) throws -> [SimulatorHIDEvent] {
        var events: [SimulatorHIDEvent] = []
        for key in try keys(for: text) {
            if key.shifted {
                events.append(.keyboard(direction: .down, keyCode: shift))
                events.append(.delay(hold))
            }
            events.append(.keyboard(direction: .down, keyCode: key.code))
            events.append(.delay(hold))
            events.append(.keyboard(direction: .up, keyCode: key.code))
            if key.shifted {
                events.append(.delay(hold))
                events.append(.keyboard(direction: .up, keyCode: shift))
            }
            events.append(.delay(pause))
        }
        return events
    }

    static func keys(for text: String) throws -> [(code: UInt32, shifted: Bool)] {
        try text.map { character in
            guard let key = table[character] else {
                throw Failure(reason: "unsupportedText", message: "Cannot type \"\(character)\" with the simulator's keyboard")
            }
            return key
        }
    }

    /// Keys with no character of their own, by the names a browser gives them in `KeyboardEvent.key`.
    static func named(_ name: String) throws -> UInt32 {
        let keys: [String: UInt32] = [
            "Enter": 40, "Escape": 41, "Backspace": 42, "Tab": 43, "Delete": 76, "Shift": shift,
            "ArrowRight": 79, "ArrowLeft": 80, "ArrowDown": 81, "ArrowUp": 82,
        ]
        guard let code = keys[name] else {
            throw Failure(reason: "badRequest", message: "Unknown key \(name). Use one of: \(keys.keys.sorted().joined(separator: ", "))")
        }
        return code
    }

    private static let table: [Character: (code: UInt32, shifted: Bool)] = {
        var table: [Character: (UInt32, Bool)] = [:]
        for (offset, letter) in "abcdefghijklmnopqrstuvwxyz".enumerated() {
            table[letter] = (UInt32(4 + offset), false)
            table[Character(letter.uppercased())] = (UInt32(4 + offset), true)
        }
        for (offset, digit) in "1234567890".enumerated() { table[digit] = (UInt32(30 + offset), false) }
        for (offset, symbol) in "!@#$%^&*()".enumerated() { table[symbol] = (UInt32(30 + offset), true) }
        let plain: [(Character, UInt32)] = [
            ("\n", 40), ("\r\n", 40), ("\r", 40), ("\u{8}", 42), ("\t", 43), (" ", 44), ("-", 45), ("=", 46), ("[", 47), ("]", 48), ("\\", 49),
            (";", 51), ("'", 52), ("`", 53), (",", 54), (".", 55), ("/", 56),
        ]
        let shifted: [(Character, UInt32)] = [
            ("_", 45), ("+", 46), ("{", 47), ("}", 48), ("|", 49), (":", 51), ("\"", 52), ("~", 53), ("<", 54), (">", 55), ("?", 56),
        ]
        for (character, code) in plain { table[character] = (code, false) }
        for (character, code) in shifted { table[character] = (code, true) }
        return table
    }()
}
