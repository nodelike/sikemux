import CoreGraphics
import XCTest

@testable import SikemuxSim

final class PortraitPointTests: XCTestCase {
    private let screen = CGSize(width: 402, height: 874)

    func testATurnedScreensCornersLandOnThePortraitScreensCorners() {
        let landscape = CGPoint(x: 874, y: 0)
        XCTAssertEqual(Simulators.portraitPoint(.zero, turned: "landscapeLeft", screen: screen), CGPoint(x: 402, y: 0))
        XCTAssertEqual(Simulators.portraitPoint(landscape, turned: "landscapeLeft", screen: screen), CGPoint(x: 402, y: 874))
        XCTAssertEqual(Simulators.portraitPoint(.zero, turned: "landscapeRight", screen: screen), CGPoint(x: 0, y: 874))
        XCTAssertEqual(Simulators.portraitPoint(landscape, turned: "landscapeRight", screen: screen), CGPoint(x: 0, y: 0))
        XCTAssertEqual(Simulators.portraitPoint(.zero, turned: "portraitUpsideDown", screen: screen), CGPoint(x: 402, y: 874))
    }

    func testPortraitPointsStayWhereTheyAre() {
        let point = CGPoint(x: 201, y: 437)
        XCTAssertEqual(Simulators.portraitPoint(point, turned: "portrait", screen: screen), point)
        XCTAssertEqual(Simulators.portraitPoint(point, turned: nil, screen: screen), point)
    }

    func testATurnedScreenIsWiderThanItIsTall() {
        XCTAssertEqual(Simulators.turnedSize(screen, "landscapeLeft"), CGSize(width: 874, height: 402))
        XCTAssertEqual(Simulators.turnedSize(screen, "landscapeRight"), CGSize(width: 874, height: 402))
        XCTAssertEqual(Simulators.turnedSize(screen, "portraitUpsideDown"), screen)
        XCTAssertEqual(Simulators.turnedSize(screen, "faceUp"), screen)
    }
}
