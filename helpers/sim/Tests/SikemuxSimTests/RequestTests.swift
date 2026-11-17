import XCTest

@testable import SikemuxSim

final class RequestTests: XCTestCase {
    func testARequestThatCannotBeReadIsAnsweredByItsId() {
        guard case let .failure(unreadable) = Request.read(#"{"id":9,"type":"tap","x":"left"}"#) else { return XCTFail() }
        XCTAssertEqual(unreadable.id, 9)
        XCTAssertTrue(unreadable.message.contains("x"), unreadable.message)
    }

    func testALineWithNoIdIsAnsweredByNone() {
        guard case let .failure(unreadable) = Request.read("not json") else { return XCTFail() }
        XCTAssertNil(unreadable.id)
    }
}
