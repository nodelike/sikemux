import Foundation

/// USB HID usage codes for the characters a US keyboard types, and whether each needs Shift.
enum Keyboard {
    static let shift: UInt32 = 225
    /// iOS drops keys sent back to back, so each one waits this long after the last.
    static let pause: TimeInterval = 0.03

    static func keys(for text: String) throws -> [(code: UInt32, shifted: Bool)] {
        try text.map { character in
            guard let key = table[character] else {
                throw Failure(reason: "unsupportedText", message: "Cannot type \"\(character)\" with the simulator's keyboard")
            }
            return key
        }
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
            ("\n", 40), ("\t", 43), (" ", 44), ("-", 45), ("=", 46), ("[", 47), ("]", 48), ("\\", 49),
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
