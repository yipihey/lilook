import SwiftUI

/// Viewer-first mobile surface.
///
/// The earlier reasoning stands: exposing lilaq's full parameter surface on a
/// phone is a poor experience regardless of toolkit. This shows the figure and
/// offers a narrow editing surface over the arguments that carry a real widget,
/// leaving everything else read-only.
@available(iOS 16.0, macOS 13.0, *)
public struct FigureView: View {
    @State private var doc: LilookDocument
    @State private var selected: Int = 0
    @State private var status: String = ""

    public init(document: LilookDocument) {
        _doc = State(initialValue: document)
    }

    public var body: some View {
        NavigationStack {
            List {
                Section("Call sites") {
                    ForEach(doc.calls) { call in
                        Button {
                            selected = call.node
                        } label: {
                            HStack {
                                Text(call.callee).monospaced()
                                if call.generated {
                                    Text("generated")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                if selected == call.node {
                                    Image(systemName: "checkmark")
                                }
                            }
                        }
                    }
                }

                if let call = doc.calls.first(where: { $0.node == selected }) {
                    Section(call.callee) {
                        ForEach(call.named, id: \.param) { arg in
                            ArgumentRow(arg: arg) { newValue in
                                do {
                                    try doc.set(node: call.node,
                                                param: arg.param,
                                                value: newValue)
                                } catch {
                                    status = error.localizedDescription
                                }
                            }
                        }
                    }
                }

                if !status.isEmpty {
                    Section("Status") { Text(status).font(.caption) }
                }
            }
            .navigationTitle("lilook")
            .toolbar {
                ToolbarItemGroup {
                    Button { doc.undo() } label: { Image(systemName: "arrow.uturn.backward") }
                        .disabled(doc.undoDepth == 0)
                    Button { doc.redo() } label: { Image(systemName: "arrow.uturn.forward") }
                }
            }
        }
    }
}

@available(iOS 16.0, macOS 13.0, *)
private struct ArgumentRow: View {
    let arg: LilookDocument.Argument
    let onChange: (String) -> Void
    @State private var buffer: String = ""

    var body: some View {
        HStack {
            Text(arg.param).font(.callout)
            Spacer()
            if arg.isEditable {
                TextField(arg.param, text: $buffer)
                    .multilineTextAlignment(.trailing)
                    .monospaced()
                    .onAppear { buffer = arg.value }
                    .onSubmit { onChange(buffer) }
            } else {
                Text(arg.value)
                    .monospaced()
                    .foregroundStyle(.secondary)
            }
        }
    }
}
