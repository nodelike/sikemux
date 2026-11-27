import FBSimulatorControl
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

    func testKeyEventsHoldBeforeReleaseAndBracketShift() throws {
        XCTAssertEqual(
            Keyboard.press(40),
            .composite([
                .keyboard(direction: .down, keyCode: 40),
                .delay(Keyboard.hold),
                .keyboard(direction: .up, keyCode: 40),
            ]))

        let plain = try Keyboard.events(for: "o-")
        XCTAssertEqual(
            plain,
            [
                .keyboard(direction: .down, keyCode: 18),
                .delay(Keyboard.hold),
                .keyboard(direction: .up, keyCode: 18),
                .delay(Keyboard.pause),
                .keyboard(direction: .down, keyCode: 45),
                .delay(Keyboard.hold),
                .keyboard(direction: .up, keyCode: 45),
                .delay(Keyboard.pause),
            ])

        let shifted = try Keyboard.events(for: "A")
        XCTAssertEqual(
            shifted,
            [
                .keyboard(direction: .down, keyCode: Keyboard.shift),
                .delay(Keyboard.hold),
                .keyboard(direction: .down, keyCode: 4),
                .delay(Keyboard.hold),
                .keyboard(direction: .up, keyCode: 4),
                .delay(Keyboard.hold),
                .keyboard(direction: .up, keyCode: Keyboard.shift),
                .delay(Keyboard.pause),
            ])
    }

    func testUnhandledCustomUrlSchemesAreRejectedWhileSystemAndInstalledSchemesPass() throws {
        var collected = Set<String>()
        Simulators.collectURLSchemes(
            from: [["CFBundleURLSchemes": ["MyApp", "com.example.App"]], ["CFBundleURLSchemes": [""]]],
            into: &collected)
        XCTAssertEqual(collected, ["myapp", "com.example.app"])

        let missing = try XCTUnwrap(URL(string: "unknownapp://open/path"))
        let error = try XCTUnwrap(Simulators.unhandledSchemeMessage(for: missing, installedSchemes: collected))
        XCTAssertTrue(error.contains("unknownapp://"), error)
        XCTAssertTrue(error.contains("sim install"), error)

        let handled = try XCTUnwrap(URL(string: "MYAPP://open/path"))
        XCTAssertNil(Simulators.unhandledSchemeMessage(for: handled, installedSchemes: collected))

        for raw in ["http://127.0.0.1:8081", "https://example.com", "tel:5551234", "mailto:a@b.com", "prefs:root=WIFI", "maps://?q=apple"] {
            let url = try XCTUnwrap(URL(string: raw))
            XCTAssertNil(Simulators.unhandledSchemeMessage(for: url, installedSchemes: []), raw)
        }

        XCTAssertNil(Simulators.unhandledSchemeMessage(for: missing, installedSchemes: nil))

        let claimants: [String: [String]] = [
            "swishxacademy": ["com.swishx.academy", "com.swishx.salesacademy"],
            "singleapp": ["com.example.single"],
        ]
        XCTAssertEqual(
            Simulators.matchingBundleIDs(for: try XCTUnwrap(URL(string: "SwishXAcademy://open")), claimants: claimants),
            ["com.swishx.academy", "com.swishx.salesacademy"])
        XCTAssertEqual(
            Simulators.matchingBundleIDs(for: try XCTUnwrap(URL(string: "singleapp://open")), claimants: claimants),
            ["com.example.single"])
        XCTAssertEqual(
            Simulators.matchingBundleIDs(for: missing, claimants: claimants),
            [])
        XCTAssertNil(Simulators.matchingBundleIDs(for: try XCTUnwrap(URL(string: "https://example.com")), claimants: claimants))
        XCTAssertNil(Simulators.matchingBundleIDs(for: missing, claimants: nil))
    }
}
