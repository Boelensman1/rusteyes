import AppKit
import CoreGraphics
import Foundation

private let protocolVersion = 1

private struct HelperContext {
    let appType: NSApplication.Type
    let mainDisplayID: CGDirectDisplayID
    let processID: Int32
}

private let context = HelperContext(
    appType: NSApplication.self,
    mainDisplayID: CGMainDisplayID(),
    processID: ProcessInfo.processInfo.processIdentifier
)

_ = context

private enum ProtocolError: Error, CustomStringConvertible {
    case invalidJSON
    case invalidMessage
    case incompatibleVersion(Int)
    case unexpectedFirstMessage(String)
    case outputEncodingFailed

    var description: String {
        switch self {
        case .invalidJSON:
            return "invalid JSON message"
        case .invalidMessage:
            return "invalid protocol message"
        case .incompatibleVersion(let version):
            return "incompatible protocol version \(version)"
        case .unexpectedFirstMessage(let type):
            return "expected hello as first message, got \(type)"
        case .outputEncodingFailed:
            return "failed to encode helper output"
        }
    }
}

private func parseMessage(_ line: String) throws -> [String: Any] {
    guard let data = line.data(using: .utf8),
          let message = try JSONSerialization.jsonObject(with: data) as? [String: Any],
          message["type"] is String
    else {
        throw ProtocolError.invalidJSON
    }

    return message
}

private func messageType(_ message: [String: Any]) throws -> String {
    guard let type = message["type"] as? String else {
        throw ProtocolError.invalidMessage
    }

    return type
}

private func writeMessage(_ message: [String: Any]) throws {
    let data = try JSONSerialization.data(withJSONObject: message)
    guard let line = String(data: data, encoding: .utf8) else {
        throw ProtocolError.outputEncodingFailed
    }

    FileHandle.standardOutput.write(Data((line + "\n").utf8))
}

private func writeError(_ error: Error) {
    let message = ["type": "error", "message": String(describing: error)]
    try? writeMessage(message)
}

private func handleHello(_ message: [String: Any]) throws {
    let version = message["version"] as? Int ?? 0
    guard version == protocolVersion else {
        throw ProtocolError.incompatibleVersion(version)
    }

    try writeMessage(["type": "ready", "version": protocolVersion])
}

private func runProtocolLoop() {
    guard let firstLine = readLine() else {
        writeError(ProtocolError.invalidMessage)
        return
    }

    do {
        let firstMessage = try parseMessage(firstLine)
        let firstType = try messageType(firstMessage)
        guard firstType == "hello" else {
            throw ProtocolError.unexpectedFirstMessage(firstType)
        }

        try handleHello(firstMessage)
    } catch {
        writeError(error)
        return
    }

    while let line = readLine() {
        do {
            let message = try parseMessage(line)
            switch try messageType(message) {
            case "startBreak", "finishBreak", "clearBreak":
                continue
            case "shutdown":
                try writeMessage(["type": "shutdownComplete"])
                return
            default:
                throw ProtocolError.invalidMessage
            }
        } catch {
            writeError(error)
        }
    }
}

runProtocolLoop()
