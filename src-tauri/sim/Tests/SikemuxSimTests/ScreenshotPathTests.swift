import XCTest

@testable import SikemuxSim

final class ScreenshotPathTests: XCTestCase {
    func testSavesUnderTheHomeFolder() throws {
        let name = "sikemux-sim-test-\(UUID().uuidString).png"
        let path = try SikemuxSim.writeScreenshot(Data([1, 2, 3]), to: "~/\(name)")
        defer { try? FileManager.default.removeItem(atPath: path) }
        XCTAssertEqual(path, NSHomeDirectory() + "/" + name)
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: path)), Data([1, 2, 3]))
    }

    func testNamesTheFolderThatIsMissing() {
        XCTAssertThrowsError(try SikemuxSim.writeScreenshot(Data(), to: "/no/such/folder/shot.png")) { error in
            XCTAssertEqual((error as? Failure)?.message, "There is no folder /no/such/folder to save the screenshot in")
        }
        XCTAssertThrowsError(try SikemuxSim.writeScreenshot(Data(), to: "relative.png"))
    }
}
