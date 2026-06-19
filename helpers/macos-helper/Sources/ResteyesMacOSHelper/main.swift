import Darwin
import Foundation
import CoreGraphics

private let protocolVersion = 2
private let directInvocationExitCode: Int32 = 2
private let directInvocationMessage =
    "resteyes-macos-helper is an internal Resteyes helper. Start Resteyes with the main resteyes binary; do not run this helper directly."
private let anyInputEventType = CGEventType(rawValue: UInt32.max)!

private enum ProtocolError: Error, CustomStringConvertible {
    case invalidJSON
    case invalidMessage
    case incompatibleVersion(Int)
    case invalidActivitySample
    case outputEncodingFailed

    var description: String {
        switch self {
        case .invalidJSON:
            return "invalid JSON message"
        case .invalidMessage:
            return "invalid protocol message"
        case .incompatibleVersion(let version):
            return "incompatible protocol version \(version)"
        case .invalidActivitySample:
            return "invalid activity sample"
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

private func writeStandardErrorLine(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}

private func exitAfterDirectInvocationMessage() -> Never {
    writeStandardErrorLine(directInvocationMessage)
    exit(directInvocationExitCode)
}

private func handleHello(_ message: [String: Any]) throws {
    let version = message["version"] as? Int ?? 0
    guard version == protocolVersion else {
        throw ProtocolError.incompatibleVersion(version)
    }

    try writeMessage(["type": "ready", "version": protocolVersion])
}

private func handlePollActivity() throws {
    let idleSeconds = CGEventSource.secondsSinceLastEventType(
        .combinedSessionState,
        eventType: anyInputEventType
    )
    guard idleSeconds.isFinite, idleSeconds >= 0 else {
        throw ProtocolError.invalidActivitySample
    }

    let idleMilliseconds = min(idleSeconds * 1000, Double(UInt64.max))
    try writeMessage([
        "type": "activitySample",
        "idleMs": UInt64(idleMilliseconds),
    ])
}

private func readInitialHelloMessage() -> [String: Any] {
    if isatty(STDIN_FILENO) != 0 {
        exitAfterDirectInvocationMessage()
    }

    do {
        guard let firstLine = readLine() else {
            exitAfterDirectInvocationMessage()
        }

        let firstMessage = try parseMessage(firstLine)
        let firstType = try messageType(firstMessage)
        guard firstType == "hello" else {
            exitAfterDirectInvocationMessage()
        }

        return firstMessage
    } catch {
        exitAfterDirectInvocationMessage()
    }
}

private func runProtocolLoop() {
    let firstMessage = readInitialHelloMessage()

    do {
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
            case "pollActivity":
                try handlePollActivity()
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
