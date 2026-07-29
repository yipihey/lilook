import XCTest
@testable import Lilook

/// Mirrors the Rust and Python suites: a gesture is one undo step, and undoing
/// it restores the buffer byte for byte.
final class LilookTests: XCTestCase {
    let source = """
    #import "@preview/lilaq:0.6.0" as lq
    #lq.diagram(width: 6cm, height: 4cm,
      lq.plot((0, 1, 2), (0, 1, 4), stroke: red),
    )
    """

    func testIndexesCallSites() throws {
        let doc = try LilookDocument(source: source)
        XCTAssertTrue(doc.calls.contains { $0.callee == "lq.plot" })
    }

    func testDragCoalescesIntoOneUndoStep() throws {
        let doc = try LilookDocument(source: source)
        let node = doc.calls.first { $0.callee == "lq.diagram" }!.node
        doc.begin(label: "drag")
        for w in ["6.5cm", "7cm", "8cm"] {
            try doc.set(node: node, param: "width", value: w)
        }
        doc.commit()
        XCTAssertTrue(doc.source.contains("width: 8cm"))
        XCTAssertEqual(doc.undoDepth, 1)
        XCTAssertTrue(doc.undo())
        XCTAssertEqual(doc.source, source)
    }

    func testUnknownParameterReportsError() throws {
        let doc = try LilookDocument(source: source)
        let node = doc.calls.first { $0.callee == "lq.plot" }!.node
        XCTAssertThrowsError(try doc.set(node: node, param: "bogus", value: "1"))
    }
}
