import XCTest

@testable import SikemuxSim

final class KeyboardTests: XCTestCase {
    func testLettersDigitsAndShiftedSymbols() throws {
        let keys = try Keyboard.keys(for: "aZ0!?\n")
        XCTAssertEqual(keys.map(\.code), [4, 29, 39, 30, 56, 40])
        XCTAssertEqual(keys.map(\.shifted), [false, true, false, true, true, false])
    }

    func testBackspaceDeletesTheCharacterBeforeTheCaret() throws {
        let keys = try Keyboard.keys(for: "\u{8}")
        XCTAssertEqual(keys.map(\.code), [42])
    }

    func testEveryPrintableAsciiCharacterIsTypable() throws {
        let printable = String((32...126).map { Character(UnicodeScalar(UInt8($0))) })
        XCTAssertEqual(try Keyboard.keys(for: printable).count, printable.count)
    }

    func testNamesTheCharacterItCannotType() {
        XCTAssertThrowsError(try Keyboard.keys(for: "é")) { error in
            guard let failure = error as? Failure else { return XCTFail("\(error)") }
            XCTAssertTrue(failure.message.contains("é"))
        }
    }

    func testEveryLineEndingTypesAsReturn() throws {
        XCTAssertEqual(try Keyboard.keys(for: "a\r\nb\rc\n").map(\.code), [4, 40, 5, 40, 6, 40])
    }
}
