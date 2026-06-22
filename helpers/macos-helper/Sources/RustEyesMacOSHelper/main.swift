import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

private let protocolVersion = 6
private let directInvocationExitCode: Int32 = 2
private let directInvocationMessage =
    "rusteyes-macos-helper is an internal RustEyes helper. Start RustEyes with the main rusteyes binary; do not run this helper directly."
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
            return "failed to create macOS input event tap; grant Accessibility and Input Monitoring permissions to RustEyes"
        case .lockScreenUnavailable(let reason):
            return "failed to lock macOS session: \(reason)"
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

        for point in screenPointCandidates(fromQuartzEventLocation: location) {
            if requestLockAfterBreak(atScreenPoint: point) {
                return
            }
        }
    }

    private func screenPointCandidates(fromQuartzEventLocation point: CGPoint) -> [CGPoint] {
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

private let messageDecoder = JSONDecoder()
private let messageEncoder = JSONEncoder()

private enum DaemonMessage {
    case hello(HelloCommand)
    case preflightPermissions
    case startBreak(StartBreakCommand)
    case updateBreak(UpdateBreakCommand)
    case finishBreak(FinishBreakCommand)
    case clearBreak
    case pollActivity
    case shutdown
}

private enum HelperCommand: String, Encodable {
    case startBreak
    case updateBreak
    case finishBreak
    case clearBreak
}

private struct MessageEnvelope: Decodable {
    let type: String
}

private struct HelloCommand: Decodable {
    let version: Int
}

private struct StartBreakCommand: Decodable {
    let breakInfo: BreakInfo

    private enum CodingKeys: String, CodingKey {
        case breakInfo = "break"
    }
}

private struct BreakInfo: Decodable {
    let durationMs: UInt64
    let messages: [String]
    let autolock: Bool

    private enum CodingKeys: String, CodingKey {
        case durationMs
        case messages
        case autolock
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        durationMs = try container.decode(UInt64.self, forKey: .durationMs)
        messages = (try? container.decode([String].self, forKey: .messages)) ?? []
        autolock = try container.decode(Bool.self, forKey: .autolock)
    }
}

private struct UpdateBreakCommand: Decodable {
    let remainingMs: UInt64
    let lockAfter: Bool
}

private struct FinishBreakCommand: Decodable {
    let lockAfter: Bool
}

private struct ReadyMessage: Encodable {
    let type = "ready"
    let version: Int
}

private struct PreflightResultMessage: Encodable {
    let type = "preflightResult"
    let accessibilityTrusted: Bool
    let inputMonitoringTrusted: Bool
}

private struct ActivitySampleMessage: Encodable {
    let type = "activitySample"
    let idleMs: UInt64
    let lockAfterBreakRequested: Bool
}

private struct CommandCompleteMessage: Encodable {
    let type = "commandComplete"
    let command: HelperCommand
}

private struct ShutdownCompleteMessage: Encodable {
    let type = "shutdownComplete"
}

private struct ErrorMessage: Encodable {
    let type = "error"
    let message: String
}

private func parseMessage(_ line: String) throws -> DaemonMessage {
    guard let data = line.data(using: .utf8) else {
        throw ProtocolError.invalidJSON
    }

    let envelope: MessageEnvelope
    do {
        envelope = try messageDecoder.decode(MessageEnvelope.self, from: data)
    } catch {
        throw ProtocolError.invalidJSON
    }

    switch envelope.type {
    case "hello":
        return .hello(try decodeMessage(HelloCommand.self, from: data))
    case "preflightPermissions":
        return .preflightPermissions
    case "startBreak":
        return .startBreak(try decodeMessage(StartBreakCommand.self, from: data))
    case "updateBreak":
        return .updateBreak(try decodeMessage(UpdateBreakCommand.self, from: data))
    case "finishBreak":
        return .finishBreak(try decodeMessage(FinishBreakCommand.self, from: data))
    case "clearBreak":
        return .clearBreak
    case "pollActivity":
        return .pollActivity
    case "shutdown":
        return .shutdown
    default:
        throw ProtocolError.invalidMessage
    }
}

private func decodeMessage<T: Decodable>(_ messageType: T.Type, from data: Data) throws -> T {
    do {
        return try messageDecoder.decode(messageType, from: data)
    } catch {
        throw ProtocolError.invalidMessage
    }
}

private func writeMessage<T: Encodable>(_ message: T) throws {
    let data = try messageEncoder.encode(message)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

private func writeError(_ error: Error) {
    try? writeMessage(ErrorMessage(message: String(describing: error)))
}

private func writeCommandComplete(_ command: HelperCommand) throws {
    try writeMessage(CommandCompleteMessage(command: command))
}

private func writeStandardErrorLine(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}

private func exitAfterDirectInvocationMessage() -> Never {
    writeStandardErrorLine(directInvocationMessage)
    exit(directInvocationExitCode)
}

private func handleHello(_ message: HelloCommand) throws {
    let version = message.version
    guard version == protocolVersion else {
        throw ProtocolError.incompatibleVersion(version)
    }

    try writeMessage(ReadyMessage(version: protocolVersion))
}

private func handlePreflightPermissions() throws {
    let accessibilityTrusted = requestAccessibilityTrust()
    let inputMonitoringTrusted = CGPreflightListenEventAccess()

    try writeMessage(PreflightResultMessage(
        accessibilityTrusted: accessibilityTrusted,
        inputMonitoringTrusted: inputMonitoringTrusted
    ))
}

private func requestAccessibilityTrust() -> Bool {
    let promptKey = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
    let options = [promptKey: true] as CFDictionary
    return AXIsProcessTrustedWithOptions(options)
}

private func handleStartBreak(_ command: StartBreakCommand, overlay: BreakOverlayController) throws {
    let state = breakOverlayState(from: command)
    try runThrowingOnMain {
        try overlay.show(state: state)
    }
}

private func handleUpdateBreak(_ command: UpdateBreakCommand, overlay: BreakOverlayController) {
    runOnMain {
        overlay.update(remainingMs: command.remainingMs, lockAfterBreak: command.lockAfter)
    }
}

private func handleFinishBreak(_ command: FinishBreakCommand, overlay: BreakOverlayController) throws {
    clearBreakOverlay(overlay)

    if command.lockAfter {
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
    try writeMessage(ActivitySampleMessage(
        idleMs: UInt64(idleMilliseconds),
        lockAfterBreakRequested: lockAfterBreakRequested
    ))
}

private func breakOverlayState(from command: StartBreakCommand) -> BreakOverlayState {
    let messages = command.breakInfo.messages
    let message = messages.first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        ?? defaultBreakMessage

    return BreakOverlayState(
        message: message,
        remainingMs: command.breakInfo.durationMs,
        lockAfterBreak: command.breakInfo.autolock
    )
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

private func readInitialHelloMessage() -> HelloCommand {
    if isatty(STDIN_FILENO) != 0 {
        exitAfterDirectInvocationMessage()
    }

    do {
        guard let firstLine = readLine() else {
            exitAfterDirectInvocationMessage()
        }

        let firstMessage = try parseMessage(firstLine)
        guard case .hello(let command) = firstMessage else {
            exitAfterDirectInvocationMessage()
        }

        return command
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
            switch message {
            case .hello:
                throw ProtocolError.invalidMessage
            case .preflightPermissions:
                try handlePreflightPermissions()
            case .startBreak(let command):
                try handleStartBreak(command, overlay: overlay)
                try writeCommandComplete(.startBreak)
            case .updateBreak(let command):
                handleUpdateBreak(command, overlay: overlay)
                try writeCommandComplete(.updateBreak)
            case .finishBreak(let command):
                try handleFinishBreak(command, overlay: overlay)
                try writeCommandComplete(.finishBreak)
            case .clearBreak:
                clearBreakOverlay(overlay)
                try writeCommandComplete(.clearBreak)
            case .pollActivity:
                try handlePollActivity(overlay: overlay)
            case .shutdown:
                clearBreakOverlay(overlay)
                try writeMessage(ShutdownCompleteMessage())
                return
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
    protocolThread.name = "rusteyes-macos-helper-protocol"
    protocolThread.start()

    RunLoop.main.run()
}

runHelper()
