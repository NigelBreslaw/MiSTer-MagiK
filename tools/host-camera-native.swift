import AVFoundation
import CoreMedia
import Foundation

struct Options {
    var command = ""
    var deviceIndex = 0
    var deviceName: String?
    var width = 1920
    var height = 1080
    var fps = 60.000240
    var duration = 10.0
    var output = "/tmp/host-camera-native.mp4"
    var bitrate = 50_000_000
}

func usage() -> Never {
    print("""
    Usage:
      scripts/host-camera-native list
      scripts/host-camera-native video [--device-index N|--device-name NAME] [--size WxH] [--fps N] [--duration SECS] [--bitrate BPS] --output PATH
    """)
    exit(2)
}

func parseOptions() -> Options {
    var args = Array(CommandLine.arguments.dropFirst())
    if args.isEmpty { usage() }
    var options = Options(command: args.removeFirst())
    while !args.isEmpty {
        let arg = args.removeFirst()
        switch arg {
        case "--device-index":
            if args.isEmpty { usage() }
            options.deviceIndex = Int(args.removeFirst()) ?? options.deviceIndex
        case "--device-name":
            if args.isEmpty { usage() }
            options.deviceName = args.removeFirst()
        case "--size":
            if args.isEmpty { usage() }
            let parts = args.removeFirst().split(separator: "x")
            if parts.count != 2 { usage() }
            options.width = Int(parts[0]) ?? options.width
            options.height = Int(parts[1]) ?? options.height
        case "--fps":
            if args.isEmpty { usage() }
            options.fps = Double(args.removeFirst()) ?? options.fps
        case "--duration":
            if args.isEmpty { usage() }
            options.duration = Double(args.removeFirst()) ?? options.duration
        case "--bitrate":
            if args.isEmpty { usage() }
            options.bitrate = Int(args.removeFirst()) ?? options.bitrate
        case "--output":
            if args.isEmpty { usage() }
            options.output = args.removeFirst()
        case "-h", "--help":
            usage()
        default:
            eprint("unknown option: \(arg)")
            usage()
        }
    }
    return options
}

func eprint(_ message: String) {
    if let data = (message + "\n").data(using: .utf8) {
        FileHandle.standardError.write(data)
    }
}

func videoDevices() -> [AVCaptureDevice] {
    var deviceTypes: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera]
    if #available(macOS 14.0, *) {
        deviceTypes.append(.external)
    } else {
        deviceTypes.append(.externalUnknown)
    }
    if #available(macOS 13.0, *) {
        deviceTypes.append(.continuityCamera)
    }
    let session = AVCaptureDevice.DiscoverySession(
        deviceTypes: deviceTypes,
        mediaType: .video,
        position: .unspecified
    )
    return session.devices
}

func dimensions(_ format: AVCaptureDevice.Format) -> CMVideoDimensions {
    CMVideoFormatDescriptionGetDimensions(format.formatDescription)
}

func listDevices() {
    for (deviceIndex, device) in videoDevices().enumerated() {
        print("[\(deviceIndex)] \(device.localizedName) uniqueID=\(device.uniqueID)")
        for format in device.formats {
            let dims = dimensions(format)
            let rates = format.videoSupportedFrameRateRanges
                .map { String(format: "%.6g-%.6g", $0.minFrameRate, $0.maxFrameRate) }
                .joined(separator: ",")
            let subtype = CMFormatDescriptionGetMediaSubType(format.formatDescription)
            let fourcc = String(bytes: [
                UInt8((subtype >> 24) & 0xff),
                UInt8((subtype >> 16) & 0xff),
                UInt8((subtype >> 8) & 0xff),
                UInt8(subtype & 0xff),
            ], encoding: .macOSRoman) ?? "????"
            print("  \(dims.width)x\(dims.height) fps=[\(rates)] pixel=\(fourcc)")
        }
    }
}

func chooseFormatAndRate(
    device: AVCaptureDevice,
    width: Int32,
    height: Int32,
    fps: Double
) -> (AVCaptureDevice.Format, AVFrameRateRange)? {
    for format in device.formats {
        let dims = dimensions(format)
        guard dims.width == width && dims.height == height else { continue }
        if let range = format.videoSupportedFrameRateRanges.first(where: { range in
            fps >= range.minFrameRate - 0.001 && fps <= range.maxFrameRate + 0.001
        }) {
            return (format, range)
        }
    }
    return nil
}

func chooseDevice(options: Options) throws -> AVCaptureDevice {
    let devices = videoDevices()
    if let name = options.deviceName {
        if let device = devices.first(where: { $0.localizedName == name }) {
            return device
        }
        throw NSError(domain: "host-camera-native", code: 2, userInfo: [NSLocalizedDescriptionKey: "device named \(name) not found"])
    }
    guard options.deviceIndex >= 0 && options.deviceIndex < devices.count else {
        throw NSError(domain: "host-camera-native", code: 2, userInfo: [NSLocalizedDescriptionKey: "device index out of range"])
    }
    return devices[options.deviceIndex]
}

final class MovieDelegate: NSObject, AVCaptureFileOutputRecordingDelegate {
    let done = DispatchSemaphore(value: 0)
    var error: Error?

    func fileOutput(
        _ output: AVCaptureFileOutput,
        didFinishRecordingTo outputFileURL: URL,
        from connections: [AVCaptureConnection],
        error: Error?
    ) {
        self.error = error
        done.signal()
    }
}

final class MovieRecorder {
    let options: Options
    let session = AVCaptureSession()
    let delegate = MovieDelegate()

    init(options: Options) throws {
        self.options = options
    }

    func start() throws {
        let device = try chooseDevice(options: options)
        guard let (format, frameRateRange) = chooseFormatAndRate(device: device, width: Int32(options.width), height: Int32(options.height), fps: options.fps) else {
            throw NSError(domain: "host-camera-native", code: 3, userInfo: [NSLocalizedDescriptionKey: "no matching \(options.width)x\(options.height)@\(options.fps) format for \(device.localizedName)"])
        }

        try device.lockForConfiguration()
        device.activeFormat = format
        device.activeVideoMinFrameDuration = frameRateRange.minFrameDuration
        device.activeVideoMaxFrameDuration = frameRateRange.maxFrameDuration
        device.unlockForConfiguration()

        session.beginConfiguration()
        let input = try AVCaptureDeviceInput(device: device)
        guard session.canAddInput(input) else {
            throw NSError(domain: "host-camera-native", code: 4, userInfo: [NSLocalizedDescriptionKey: "cannot add camera input"])
        }
        session.addInput(input)

        let output = AVCaptureMovieFileOutput()
        output.movieFragmentInterval = .invalid
        guard session.canAddOutput(output) else {
            throw NSError(domain: "host-camera-native", code: 5, userInfo: [NSLocalizedDescriptionKey: "cannot add movie output"])
        }
        session.addOutput(output)
        session.commitConfiguration()

        let outputURL = URL(fileURLWithPath: options.output)
        try? FileManager.default.removeItem(at: outputURL)
        print("recording \(device.localizedName) \(options.width)x\(options.height)@\(frameRateRange.maxFrameRate) -> \(options.output)")
        session.startRunning()
        output.startRecording(to: outputURL, recordingDelegate: delegate)
        Thread.sleep(forTimeInterval: options.duration)
        output.stopRecording()
        _ = delegate.done.wait(timeout: .now() + 15.0)
        session.stopRunning()
        if let error = delegate.error {
            let nsError = error as NSError
            if nsError.domain != AVFoundationErrorDomain || nsError.code != AVError.Code.maximumDurationReached.rawValue {
                throw error
            }
        }
    }
}

let options = parseOptions()
switch options.command {
case "list":
    listDevices()
case "video":
    do {
        try MovieRecorder(options: options).start()
    } catch {
        eprint("host-camera-native: \(error.localizedDescription)")
        exit(1)
    }
default:
    usage()
}
