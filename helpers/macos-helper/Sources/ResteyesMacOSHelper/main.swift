import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

private let protocolVersion = 4
private let directInvocationExitCode: Int32 = 2
private let directInvocationMessage =
    "resteyes-macos-helper is an internal Resteyes helper. Start Resteyes with the main resteyes binary; do not run this helper directly."
private let anyInputEventType = CGEventType(rawValue: UInt32.max)!
private let defaultBreakMessage = "Take a break"

private enum ProtocolError: Error, CustomStringConvertible {
    case invalidJSON
    case invalidMessage
    case incompatibleVersion(Int)
    case invalidActivitySample
    case inputBlockingUnavailable
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
        case .inputBlockingUnavailable:
            return "failed to create macOS input event tap; grant Accessibility and Input Monitoring permissions to Resteyes"
        case .outputEncodingFailed:
            return "failed to encode helper output"
        }
    }
}

private final class InputBlocker {
    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    func enable() throws {
        guard eventTap == nil else {
            return
        }

        let userInfo = Unmanaged.passUnretained(self).toOpaque()
        guard let eventTap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: inputBlockingEventMask(),
            callback: inputBlockingEventCallback,
            userInfo: userInfo
        ) else {
            throw ProtocolError.inputBlockingUnavailable
        }

        guard let runLoopSource = CFMachPortCreateRunLoopSource(
            kCFAllocatorDefault,
            eventTap,
            0
        ) else {
            CGEvent.tapEnable(tap: eventTap, enable: false)
            throw ProtocolError.inputBlockingUnavailable
        }

        self.eventTap = eventTap
        self.runLoopSource = runLoopSource

        CFRunLoopAddSource(CFRunLoopGetMain(), runLoopSource, .commonModes)
        CGEvent.tapEnable(tap: eventTap, enable: true)
    }

    func disable() {
        guard let eventTap else {
            return
        }

        CGEvent.tapEnable(tap: eventTap, enable: false)
        if let runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), runLoopSource, .commonModes)
        }

        self.eventTap = nil
        self.runLoopSource = nil
    }

    func reenableIfActive() {
        guard let eventTap else {
            return
        }

        CGEvent.tapEnable(tap: eventTap, enable: true)
    }
}

private final class BreakOverlayController {
    private var windows: [NSWindow] = []
    private let inputBlocker = InputBlocker()
    private var applicationPrepared = false

    func show(message: String) throws {
        prepareApplication()
        clear()
        try inputBlocker.enable()

        for screen in NSScreen.screens {
            let window = overlayWindow(for: screen, message: message)
            windows.append(window)
            window.orderFrontRegardless()
        }
    }

    func clear() {
        for window in windows {
            window.orderOut(nil)
            window.close()
        }
        windows.removeAll()
        inputBlocker.disable()
    }

    private func overlayWindow(for screen: NSScreen, message: String) -> NSWindow {
        let frame = screen.frame
        let window = NSWindow(
            contentRect: frame,
            styleMask: .borderless,
            backing: .buffered,
            defer: false,
            screen: screen
        )
        window.backgroundColor = .black
        window.collectionBehavior = [
            .canJoinAllSpaces,
            .fullScreenAuxiliary,
            .ignoresCycle,
            .stationary,
        ]
        window.contentView = BreakOverlayView(
            frame: NSRect(origin: .zero, size: frame.size),
            message: message
        )
        window.hasShadow = false
        window.ignoresMouseEvents = false
        window.isOpaque = true
        window.isReleasedWhenClosed = false
        window.level = .screenSaver
        return window
    }

    private func prepareApplication() {
        guard !applicationPrepared else {
            return
        }

        let application = NSApplication.shared
        application.setActivationPolicy(.accessory)
        application.finishLaunching()
        applicationPrepared = true
    }
}

private final class BreakOverlayView: NSView {
    private let message: String

    init(frame frameRect: NSRect, message: String) {
        self.message = message
        super.init(frame: frameRect)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("BreakOverlayView does not support coder initialization")
    }

    override var isOpaque: Bool {
        true
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.black.setFill()
        dirtyRect.fill()
        drawMessage()
    }

    private func drawMessage() {
        let availableWidth = max(bounds.width * 0.8, 1)
        let availableHeight = max(bounds.height * 0.8, 1)
        let paragraphStyle = NSMutableParagraphStyle()
        paragraphStyle.alignment = .center

        var fontSize = min(max(min(bounds.width, bounds.height) / 12, 24), 72)
        var renderedMessage = attributedMessage(fontSize: fontSize, paragraphStyle: paragraphStyle)
        var textBounds = measuredBounds(for: renderedMessage, width: availableWidth)

        while textBounds.height > availableHeight && fontSize > 18 {
            fontSize -= 2
            renderedMessage = attributedMessage(fontSize: fontSize, paragraphStyle: paragraphStyle)
            textBounds = measuredBounds(for: renderedMessage, width: availableWidth)
        }

        let textHeight = min(textBounds.height.rounded(.up), availableHeight)
        let textRect = NSRect(
            x: bounds.midX - availableWidth / 2,
            y: bounds.midY - textHeight / 2,
            width: availableWidth,
            height: textHeight
        )
        renderedMessage.draw(
            with: textRect,
            options: [.usesLineFragmentOrigin, .usesFontLeading]
        )
    }

    private func attributedMessage(
        fontSize: CGFloat,
        paragraphStyle: NSParagraphStyle
    ) -> NSAttributedString {
        NSAttributedString(
            string: message,
            attributes: [
                .font: NSFont.systemFont(ofSize: fontSize, weight: .medium),
                .foregroundColor: NSColor.white,
                .paragraphStyle: paragraphStyle,
            ]
        )
    }

    private func measuredBounds(for message: NSAttributedString, width: CGFloat) -> NSRect {
        message.boundingRect(
            with: NSSize(width: width, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading]
        )
    }
}

private func inputBlockingEventMask() -> CGEventMask {
    inputBlockingEventTypes().reduce(CGEventMask(0)) { eventMask, eventType in
        eventMask | (CGEventMask(1) << eventType.rawValue)
    }
}

private func inputBlockingEventTypes() -> [CGEventType] {
    [
        .leftMouseDown,
        .leftMouseUp,
        .rightMouseDown,
        .rightMouseUp,
        .mouseMoved,
        .leftMouseDragged,
        .rightMouseDragged,
        .keyDown,
        .keyUp,
        .flagsChanged,
        .scrollWheel,
        .tabletPointer,
        .tabletProximity,
        .otherMouseDown,
        .otherMouseUp,
        .otherMouseDragged,
    ]
}

private func inputBlockingEventCallback(
    _: CGEventTapProxy,
    type: CGEventType,
    event: CGEvent,
    userInfo: UnsafeMutableRawPointer?
) -> Unmanaged<CGEvent>? {
    if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
        if let userInfo {
            let inputBlocker = Unmanaged<InputBlocker>
                .fromOpaque(userInfo)
                .takeUnretainedValue()
            inputBlocker.reenableIfActive()
        }

        return Unmanaged.passUnretained(event)
    }

    return nil
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

private func writeCommandComplete(_ command: String) throws {
    try writeMessage(["type": "commandComplete", "command": command])
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

private func handlePreflightPermissions() throws {
    let accessibilityTrusted = requestAccessibilityTrust()
    let inputMonitoringTrusted = CGPreflightListenEventAccess()

    try writeMessage([
        "type": "preflightResult",
        "accessibilityTrusted": accessibilityTrusted,
        "inputMonitoringTrusted": inputMonitoringTrusted,
    ])
}

private func requestAccessibilityTrust() -> Bool {
    let promptKey = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
    let options = [promptKey: true] as CFDictionary
    return AXIsProcessTrustedWithOptions(options)
}

private func handleStartBreak(_ message: [String: Any], overlay: BreakOverlayController) throws {
    let message = try selectedBreakMessage(from: message)
    try runThrowingOnMain {
        try overlay.show(message: message)
    }
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

private func selectedBreakMessage(from message: [String: Any]) throws -> String {
    guard let breakInfo = message["break"] as? [String: Any] else {
        throw ProtocolError.invalidMessage
    }

    let messages = breakInfo["messages"] as? [String] ?? []
    return messages.first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        ?? defaultBreakMessage
}

private func clearBreakOverlay(_ overlay: BreakOverlayController) {
    runOnMain {
        overlay.clear()
    }
}

private func runOnMain(_ body: @escaping () -> Void) {
    if Thread.isMainThread {
        body()
    } else {
        DispatchQueue.main.sync(execute: body)
    }
}

private func runThrowingOnMain(_ body: @escaping () throws -> Void) throws {
    if Thread.isMainThread {
        try body()
    } else {
        var thrownError: Error?
        DispatchQueue.main.sync {
            do {
                try body()
            } catch {
                thrownError = error
            }
        }
        if let thrownError {
            throw thrownError
        }
    }
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

private func runProtocolLoop(overlay: BreakOverlayController) {
    let firstMessage = readInitialHelloMessage()

    do {
        try handleHello(firstMessage)
    } catch {
        writeError(error)
        return
    }

    defer {
        clearBreakOverlay(overlay)
    }

    while let line = readLine() {
        do {
            let message = try parseMessage(line)
            switch try messageType(message) {
            case "preflightPermissions":
                try handlePreflightPermissions()
            case "startBreak":
                try handleStartBreak(message, overlay: overlay)
                try writeCommandComplete("startBreak")
            case "finishBreak":
                clearBreakOverlay(overlay)
                try writeCommandComplete("finishBreak")
            case "clearBreak":
                clearBreakOverlay(overlay)
                try writeCommandComplete("clearBreak")
            case "pollActivity":
                try handlePollActivity()
            case "shutdown":
                clearBreakOverlay(overlay)
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

private func runHelper() {
    let overlay = BreakOverlayController()
    let protocolThread = Thread {
        runProtocolLoop(overlay: overlay)
        DispatchQueue.main.async {
            exit(EXIT_SUCCESS)
        }
    }
    protocolThread.name = "resteyes-macos-helper-protocol"
    protocolThread.start()

    RunLoop.main.run()
}

runHelper()
