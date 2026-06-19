import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

private let protocolVersion = 6
private let directInvocationExitCode: Int32 = 2
private let directInvocationMessage =
    "resteyes-macos-helper is an internal Resteyes helper. Start Resteyes with the main resteyes binary; do not run this helper directly."
private let anyInputEventType = CGEventType(rawValue: UInt32.max)!
private let defaultBreakMessage = "Take a break"
private let lockControlLabel = "Lock after break"
private let lockControlRequestedLabel = "Locking after break"
private let loginFrameworkPath = "/System/Library/PrivateFrameworks/login.framework/Versions/Current/login"
private let lockScreenSymbolName = "SACLockScreenImmediate"
private typealias LockScreenImmediate = @convention(c) () -> Void

private enum ProtocolError: Error, CustomStringConvertible {
    case invalidJSON
    case invalidMessage
    case incompatibleVersion(Int)
    case invalidActivitySample
    case inputBlockingUnavailable
    case lockScreenUnavailable(String)
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
        case .lockScreenUnavailable(let reason):
            return "failed to lock macOS session: \(reason)"
        case .outputEncodingFailed:
            return "failed to encode helper output"
        }
    }
}

private struct BreakOverlayState {
    var message: String
    var remainingMs: UInt64
    var lockAfterBreak: Bool
}

private final class InputBlocker {
    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?
    var mouseDownHandler: ((CGPoint) -> Void)?

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

    func handle(type: CGEventType, event: CGEvent) -> Unmanaged<CGEvent>? {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            reenableIfActive()
            return Unmanaged.passUnretained(event)
        }

        if isMouseDownEvent(type) {
            let location = event.location
            if Thread.isMainThread {
                mouseDownHandler?(location)
            } else {
                DispatchQueue.main.async { [weak self] in
                    self?.mouseDownHandler?(location)
                }
            }
        }

        return nil
    }
}

private final class BreakOverlayController {
    private var windows: [NSWindow] = []
    private let inputBlocker = InputBlocker()
    private var applicationPrepared = false
    private var state: BreakOverlayState?
    private var lockAfterBreakRequested = false

    func show(state: BreakOverlayState) throws {
        prepareApplication()
        clear()
        self.state = state
        lockAfterBreakRequested = false
        inputBlocker.mouseDownHandler = { [weak self] location in
            self?.handleMouseDown(at: location)
        }
        try inputBlocker.enable()

        for screen in NSScreen.screens {
            let window = overlayWindow(for: screen, state: state)
            windows.append(window)
            window.orderFrontRegardless()
        }
        NSCursor.arrow.set()
    }

    func update(remainingMs: UInt64, lockAfterBreak: Bool) {
        guard var state else {
            return
        }

        state.remainingMs = remainingMs
        state.lockAfterBreak = lockAfterBreak
        self.state = state
        updateWindows(with: state)
    }

    func takeLockAfterBreakRequest() -> Bool {
        let requested = lockAfterBreakRequested
        lockAfterBreakRequested = false
        return requested
    }

    func clear() {
        for window in windows {
            window.orderOut(nil)
            window.close()
        }
        windows.removeAll()
        inputBlocker.disable()
        inputBlocker.mouseDownHandler = nil
        state = nil
        lockAfterBreakRequested = false
    }

    private func overlayWindow(for screen: NSScreen, state: BreakOverlayState) -> NSWindow {
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
            state: state
        )
        window.hasShadow = false
        window.ignoresMouseEvents = false
        window.isOpaque = true
        window.isReleasedWhenClosed = false
        window.level = .screenSaver
        return window
    }

    private func handleMouseDown(at location: CGPoint) {
        guard let state, !state.lockAfterBreak else {
            return
        }

        for point in candidateScreenPoints(for: location) {
            if requestLockAfterBreak(atScreenPoint: point) {
                return
            }
        }
    }

    private func candidateScreenPoints(for point: CGPoint) -> [CGPoint] {
        let flippedPoint = flippedScreenPoint(point)
        if abs(flippedPoint.x - point.x) < 0.5 && abs(flippedPoint.y - point.y) < 0.5 {
            return [point]
        }

        return [point, flippedPoint]
    }

    private func flippedScreenPoint(_ point: CGPoint) -> CGPoint {
        let frame = combinedScreenFrame()
        return CGPoint(x: point.x, y: frame.maxY - (point.y - frame.minY))
    }

    private func combinedScreenFrame() -> NSRect {
        guard var frame = NSScreen.screens.first?.frame else {
            return .zero
        }

        for screen in NSScreen.screens.dropFirst() {
            frame = frame.union(screen.frame)
        }

        return frame
    }

    private func requestLockAfterBreak(atScreenPoint point: CGPoint) -> Bool {
        for window in windows {
            guard let view = window.contentView as? BreakOverlayView else {
                continue
            }

            let localPoint = CGPoint(
                x: point.x - window.frame.minX,
                y: point.y - window.frame.minY
            )
            if view.bounds.contains(localPoint), view.lockControlContains(localPoint) {
                markLockAfterBreakRequested()
                return true
            }
        }

        return false
    }

    private func markLockAfterBreakRequested() {
        guard var state, !state.lockAfterBreak else {
            return
        }

        state.lockAfterBreak = true
        self.state = state
        lockAfterBreakRequested = true
        updateWindows(with: state)
    }

    private func updateWindows(with state: BreakOverlayState) {
        for window in windows {
            if let view = window.contentView as? BreakOverlayView {
                view.update(state: state)
            }
        }
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
    private var state: BreakOverlayState

    init(frame frameRect: NSRect, state: BreakOverlayState) {
        self.state = state
        super.init(frame: frameRect)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("BreakOverlayView does not support coder initialization")
    }

    override var isOpaque: Bool {
        true
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .arrow)
    }

    func update(state: BreakOverlayState) {
        self.state = state
        needsDisplay = true
    }

    func lockControlContains(_ point: CGPoint) -> Bool {
        overlayLayout().lockControlRect.contains(point)
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.black.setFill()
        dirtyRect.fill()
        let layout = overlayLayout()

        drawMessage(in: layout.messageRect, fontSize: layout.messageFontSize)
        drawRemainingTime(in: layout.remainingRect)
        drawLockControl(layout)
    }

    private func drawMessage(in rect: NSRect, fontSize: CGFloat) {
        attributedMessage(fontSize: fontSize).draw(
            with: rect,
            options: [.usesLineFragmentOrigin, .usesFontLeading]
        )
    }

    private func drawRemainingTime(in rect: NSRect) {
        attributedCenteredText(
            remainingTimeText(state.remainingMs),
            font: NSFont.monospacedDigitSystemFont(ofSize: 36, weight: .regular),
            color: .white
        )
        .draw(with: rect, options: [.usesLineFragmentOrigin, .usesFontLeading])
    }

    private func drawLockControl(_ layout: OverlayLayout) {
        let path = NSBezierPath(
            roundedRect: layout.lockControlRect,
            xRadius: 4,
            yRadius: 4
        )
        let textColor: NSColor

        if state.lockAfterBreak {
            NSColor.white.setFill()
            path.fill()
            textColor = .black
        } else {
            NSColor.white.setStroke()
            path.lineWidth = 1
            path.stroke()
            textColor = .white
        }

        attributedCenteredText(
            lockControlText(),
            font: NSFont.systemFont(ofSize: layout.lockControlFontSize, weight: .medium),
            color: textColor
        )
        .draw(
            with: layout.lockControlTextRect,
            options: [.usesLineFragmentOrigin, .usesFontLeading]
        )
    }

    private func overlayLayout() -> OverlayLayout {
        let availableWidth = max(bounds.width * 0.8, 1)
        let availableHeight = max(bounds.height * 0.8, 1)
        let verticalGap: CGFloat = 18
        let remainingHeight: CGFloat = 42
        let controlHeight: CGFloat = 40
        let maxMessageHeight = max(
            availableHeight - remainingHeight - controlHeight - verticalGap * 2,
            20
        )

        var fontSize = min(max(min(bounds.width, bounds.height) / 12, 24), 72)
        var renderedMessage = attributedMessage(fontSize: fontSize)
        var textBounds = measuredBounds(for: renderedMessage, width: availableWidth)

        while textBounds.height > maxMessageHeight && fontSize > 18 {
            fontSize -= 2
            renderedMessage = attributedMessage(fontSize: fontSize)
            textBounds = measuredBounds(for: renderedMessage, width: availableWidth)
        }

        let messageHeight = min(max(textBounds.height.rounded(.up), 20), maxMessageHeight)
        let totalHeight = messageHeight + remainingHeight + controlHeight + verticalGap * 2
        let top = bounds.midY + totalHeight / 2
        let messageRect = NSRect(
            x: bounds.midX - availableWidth / 2,
            y: top - messageHeight,
            width: availableWidth,
            height: messageHeight
        )
        let remainingRect = NSRect(
            x: bounds.midX - availableWidth / 2,
            y: messageRect.minY - verticalGap - remainingHeight,
            width: availableWidth,
            height: remainingHeight
        )
        let controlFontSize = lockControlFontSize(maxWidth: availableWidth)
        let controlWidth = lockControlWidth(maxWidth: availableWidth, fontSize: controlFontSize)
        let controlRect = NSRect(
            x: bounds.midX - controlWidth / 2,
            y: remainingRect.minY - verticalGap - controlHeight,
            width: controlWidth,
            height: controlHeight
        )
        let textRect = controlRect.insetBy(dx: 16, dy: 9)

        return OverlayLayout(
            messageRect: messageRect,
            messageFontSize: fontSize,
            remainingRect: remainingRect,
            lockControlRect: controlRect,
            lockControlTextRect: textRect,
            lockControlFontSize: controlFontSize
        )
    }

    private func lockControlWidth(maxWidth: CGFloat, fontSize: CGFloat) -> CGFloat {
        let text = attributedCenteredText(
            lockControlText(),
            font: NSFont.systemFont(ofSize: fontSize, weight: .medium),
            color: .white
        )
        let textWidth = measuredBounds(for: text, width: .greatestFiniteMagnitude).width
        return min(max(textWidth + 32, 160), maxWidth)
    }

    private func lockControlFontSize(maxWidth: CGFloat) -> CGFloat {
        var fontSize: CGFloat = 18
        while fontSize > 12 {
            let font = NSFont.systemFont(ofSize: fontSize, weight: .medium)
            let text = attributedCenteredText(lockControlText(), font: font, color: .white)
            let textWidth = measuredBounds(for: text, width: .greatestFiniteMagnitude).width
            if textWidth + 32 <= maxWidth {
                return fontSize
            }
            fontSize -= 1
        }

        return fontSize
    }

    private func attributedMessage(fontSize: CGFloat) -> NSAttributedString {
        attributedCenteredText(
            state.message,
            font: NSFont.systemFont(ofSize: fontSize, weight: .medium),
            color: .white
        )
    }

    private func attributedCenteredText(
        _ text: String,
        font: NSFont,
        color: NSColor
    ) -> NSAttributedString {
        let paragraphStyle = NSMutableParagraphStyle()
        paragraphStyle.alignment = .center

        return NSAttributedString(
            string: text,
            attributes: [
                .font: font,
                .foregroundColor: color,
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

    private func lockControlText() -> String {
        state.lockAfterBreak ? lockControlRequestedLabel : lockControlLabel
    }
}

private struct OverlayLayout {
    let messageRect: NSRect
    let messageFontSize: CGFloat
    let remainingRect: NSRect
    let lockControlRect: NSRect
    let lockControlTextRect: NSRect
    let lockControlFontSize: CGFloat
}

private func remainingTimeText(_ remainingMs: UInt64) -> String {
    let seconds = remainingMs / 1_000 + (remainingMs % 1_000 == 0 ? 0 : 1)
    let hours = seconds / 3_600
    let minutes = (seconds % 3_600) / 60
    let remainingSeconds = seconds % 60

    if hours > 0 {
        return "\(hours):\(paddedTimeComponent(minutes)):\(paddedTimeComponent(remainingSeconds))"
    }

    return "\(minutes):\(paddedTimeComponent(remainingSeconds))"
}

private func paddedTimeComponent(_ value: UInt64) -> String {
    value < 10 ? "0\(value)" : "\(value)"
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
    guard let userInfo else {
        return nil
    }

    let inputBlocker = Unmanaged<InputBlocker>
        .fromOpaque(userInfo)
        .takeUnretainedValue()
    return inputBlocker.handle(type: type, event: event)
}

private func isMouseDownEvent(_ type: CGEventType) -> Bool {
    type == .leftMouseDown || type == .rightMouseDown || type == .otherMouseDown
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
    let state = try breakOverlayState(from: message)
    try runThrowingOnMain {
        try overlay.show(state: state)
    }
}

private func handleUpdateBreak(_ message: [String: Any], overlay: BreakOverlayController) throws {
    let remainingMs = try unsignedInteger(message["remainingMs"])
    let lockAfterBreak = try lockAfterBreak(from: message)
    runOnMain {
        overlay.update(remainingMs: remainingMs, lockAfterBreak: lockAfterBreak)
    }
}

private func handleFinishBreak(_ message: [String: Any], overlay: BreakOverlayController) throws {
    let lockAfter = try lockAfterBreak(from: message)
    clearBreakOverlay(overlay)

    if lockAfter {
        try lockScreenImmediately()
    }
}

private func handlePollActivity(overlay: BreakOverlayController) throws {
    let idleSeconds = CGEventSource.secondsSinceLastEventType(
        .combinedSessionState,
        eventType: anyInputEventType
    )
    guard idleSeconds.isFinite, idleSeconds >= 0 else {
        throw ProtocolError.invalidActivitySample
    }

    let idleMilliseconds = min(idleSeconds * 1000, Double(UInt64.max))
    let lockAfterBreakRequested = runReturningOnMain {
        overlay.takeLockAfterBreakRequest()
    }
    try writeMessage([
        "type": "activitySample",
        "idleMs": UInt64(idleMilliseconds),
        "lockAfterBreakRequested": lockAfterBreakRequested,
    ])
}

private func breakOverlayState(from message: [String: Any]) throws -> BreakOverlayState {
    guard let breakInfo = message["break"] as? [String: Any] else {
        throw ProtocolError.invalidMessage
    }

    let messages = breakInfo["messages"] as? [String] ?? []
    let message = messages.first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        ?? defaultBreakMessage
    let durationMs = try unsignedInteger(breakInfo["durationMs"])
    let autolock = try booleanValue(breakInfo["autolock"])

    return BreakOverlayState(
        message: message,
        remainingMs: durationMs,
        lockAfterBreak: autolock
    )
}

private func unsignedInteger(_ value: Any?) throws -> UInt64 {
    guard let number = value as? NSNumber else {
        throw ProtocolError.invalidMessage
    }

    let doubleValue = number.doubleValue
    guard doubleValue.isFinite, doubleValue >= 0, doubleValue.rounded() == doubleValue else {
        throw ProtocolError.invalidMessage
    }

    return number.uint64Value
}

private func booleanValue(_ value: Any?) throws -> Bool {
    guard let value = value as? Bool else {
        throw ProtocolError.invalidMessage
    }

    return value
}

private func lockAfterBreak(from message: [String: Any]) throws -> Bool {
    guard let lockAfter = message["lockAfter"] as? Bool else {
        throw ProtocolError.invalidMessage
    }

    return lockAfter
}

private func lockScreenImmediately() throws {
    clearDynamicLoaderError()
    guard let handle = dlopen(loginFrameworkPath, RTLD_LAZY) else {
        throw ProtocolError.lockScreenUnavailable(
            "could not open \(loginFrameworkPath): \(dynamicLoaderError())"
        )
    }
    defer {
        _ = dlclose(handle)
    }

    clearDynamicLoaderError()
    guard let symbol = dlsym(handle, lockScreenSymbolName) else {
        throw ProtocolError.lockScreenUnavailable(
            "could not resolve \(lockScreenSymbolName): \(dynamicLoaderError())"
        )
    }

    let lockScreen = unsafeBitCast(symbol, to: LockScreenImmediate.self)
    lockScreen()
}

private func clearDynamicLoaderError() {
    _ = dlerror()
}

private func dynamicLoaderError() -> String {
    guard let error = dlerror() else {
        return "no dynamic loader error detail"
    }

    return String(cString: error)
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

private func runReturningOnMain<T>(_ body: @escaping () -> T) -> T {
    if Thread.isMainThread {
        return body()
    }

    var result: T?
    DispatchQueue.main.sync {
        result = body()
    }
    return result!
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
            case "updateBreak":
                try handleUpdateBreak(message, overlay: overlay)
                try writeCommandComplete("updateBreak")
            case "finishBreak":
                try handleFinishBreak(message, overlay: overlay)
                try writeCommandComplete("finishBreak")
            case "clearBreak":
                clearBreakOverlay(overlay)
                try writeCommandComplete("clearBreak")
            case "pollActivity":
                try handlePollActivity(overlay: overlay)
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
