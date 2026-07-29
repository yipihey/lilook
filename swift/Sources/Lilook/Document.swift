import CLilook
import Foundation

/// Swift wrapper over the lilook C ABI.
///
/// The Typst source is the model, so this type owns a handle to a Rust-side
/// document and never reconstructs source from a Swift-side object graph.
/// Intents cross the boundary as JSON, so adding an intent needs no change
/// here beyond a convenience method.
public final class LilookDocument {
    private let handle: OpaquePointer

    public enum Failure: Error, LocalizedError {
        case couldNotCreate
        case apply(String)

        public var errorDescription: String? {
            switch self {
            case .couldNotCreate: return "could not create document"
            case .apply(let m): return m
            }
        }
    }

    public init(source: String) throws {
        guard let h = source.withCString({ lilook_doc_new($0) }) else {
            throw Failure.couldNotCreate
        }
        handle = OpaquePointer(h)
    }

    deinit {
        lilook_doc_free(UnsafeMutablePointer(handle))
    }

    /// Copies an owned C string out and releases it.
    private static func take(_ p: UnsafeMutablePointer<CChar>?) -> String? {
        guard let p else { return nil }
        defer { lilook_string_free(p) }
        return String(cString: p)
    }

    public var source: String {
        Self.take(lilook_doc_text(UnsafePointer(handle))) ?? ""
    }

    // MARK: - Call sites

    public struct Argument: Decodable, Sendable {
        public let param: String
        public let value: String
        public let editability: String

        /// Bound identifiers and computed expressions are shown, not edited.
        public var isEditable: Bool {
            editability == "literal" || editability == "builtin"
        }
    }

    public struct CallSite: Decodable, Sendable, Identifiable {
        public let node: Int
        public let callee: String
        public let generated: Bool
        public let positional: Int
        public let named: [Argument]
        public var id: Int { node }
    }

    public var calls: [CallSite] {
        guard let json = Self.take(lilook_doc_calls_json(UnsafePointer(handle))),
              let data = json.data(using: .utf8),
              let decoded = try? JSONDecoder().decode([CallSite].self, from: data)
        else { return [] }
        return decoded
    }

    // MARK: - Transactions

    /// Open a coalescing transaction. A whole gesture becomes one undo step,
    /// which is why this is exposed rather than inferred per edit. Which edits
    /// collapse together is decided per intent by the core.
    public func begin(label: String) {
        label.withCString { l in
            lilook_doc_begin(UnsafeMutablePointer(handle), l)
        }
    }

    public func commit() {
        lilook_doc_commit(UnsafeMutablePointer(handle))
    }

    private func apply(_ intent: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: intent)
        let json = String(decoding: data, as: UTF8.self)
        var err: UnsafeMutablePointer<CChar>?
        let rc = json.withCString {
            lilook_doc_apply_json(UnsafeMutablePointer(handle), $0, &err)
        }
        if rc != 0 {
            throw Failure.apply(Self.take(err) ?? "apply failed (\(rc))")
        }
    }

    public func set(node: Int, param: String, value: String) throws {
        try apply(["op": "set-named-arg", "node": node, "param": param, "value": value])
    }

    public func add(node: Int, param: String, value: String) throws {
        try apply(["op": "insert-named-arg", "node": node, "param": param, "value": value])
    }

    @discardableResult
    public func undo() -> Bool {
        lilook_doc_undo(UnsafeMutablePointer(handle)) == 1
    }

    @discardableResult
    public func redo() -> Bool {
        lilook_doc_redo(UnsafeMutablePointer(handle)) == 1
    }

    public var undoDepth: Int {
        Int(lilook_doc_undo_depth(UnsafePointer(handle)))
    }
}
