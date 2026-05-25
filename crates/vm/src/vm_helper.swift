// Nebo VM Helper — Virtualization.framework wrapper
//
// Creates and manages lightweight Linux VMs on macOS using Apple's
// Virtualization framework. Communication with the guest daemon happens
// via virtio serial (mapped to this process's stdin/stdout), so the
// host Rust code can pipe RPC messages directly through us.
//
// Usage:
//   nebo-vm-helper start --memory-mb 2048 --cpus 2 --image /path/to/vm.bundle
//   nebo-vm-helper create --image /path/to/vm.bundle --disk-gb 10
//   nebo-vm-helper status --image /path/to/vm.bundle
//
// The "start" command runs the VM and bridges stdin/stdout to a virtio
// serial port connected to the guest. This keeps the protocol identical
// to the length-prefixed JSON wire format used everywhere else.

import Foundation
import Virtualization

// MARK: - CLI Argument Parsing

struct Config {
    var action: String = "start"
    var memoryMB: UInt64 = 2048
    var cpuCount: Int = 2
    var imagePath: String = ""
    var diskSizeGB: Int = 10
}

func parseArgs() -> Config {
    var config = Config()
    let args = CommandLine.arguments
    var i = 1
    while i < args.count {
        switch args[i] {
        case "start", "create", "stop", "status":
            config.action = args[i]
        case "--memory-mb":
            i += 1
            config.memoryMB = UInt64(args[i]) ?? 2048
        case "--cpus":
            i += 1
            config.cpuCount = Int(args[i]) ?? 2
        case "--image":
            i += 1
            config.imagePath = args[i]
        case "--disk-gb":
            i += 1
            config.diskSizeGB = Int(args[i]) ?? 10
        default:
            break
        }
        i += 1
    }
    return config
}

// MARK: - VM Bundle Paths

struct VMBundle {
    let path: String

    var kernelPath: String { "\(path)/vmlinux" }
    var initrdPath: String { "\(path)/initrd" }
    var diskPath: String { "\(path)/disk.img" }
    var efiVarsPath: String { "\(path)/efivars.fd" }

    func ensureExists() throws {
        let fm = FileManager.default
        if !fm.fileExists(atPath: path) {
            try fm.createDirectory(atPath: path, withIntermediateDirectories: true)
        }
    }
}

// MARK: - VM Configuration

func createVMConfig(config: Config, bundle: VMBundle) throws -> VZVirtualMachineConfiguration {
    let vmConfig = VZVirtualMachineConfiguration()

    // CPU & Memory
    vmConfig.cpuCount = max(config.cpuCount, VZVirtualMachineConfiguration.minimumAllowedCPUCount)
    vmConfig.memorySize = config.memoryMB * 1024 * 1024

    // Boot loader — use EFI for standard Linux boot
    let bootLoader = VZEFIBootLoader()
    if FileManager.default.fileExists(atPath: bundle.efiVarsPath) {
        bootLoader.variableStore = VZEFIVariableStore(url: URL(fileURLWithPath: bundle.efiVarsPath))
    } else {
        bootLoader.variableStore = try VZEFIVariableStore(
            creatingVariableStoreAt: URL(fileURLWithPath: bundle.efiVarsPath),
            options: []
        )
    }
    vmConfig.bootLoader = bootLoader

    // Platform — generic Linux
    let platform = VZGenericPlatformConfiguration()
    vmConfig.platform = platform

    // Disk
    if FileManager.default.fileExists(atPath: bundle.diskPath) {
        let diskAttachment = try VZDiskImageStorageDeviceAttachment(
            url: URL(fileURLWithPath: bundle.diskPath),
            readOnly: false
        )
        vmConfig.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: diskAttachment)]
    }

    // Network — NAT (VM gets internet via host)
    let networkDevice = VZVirtioNetworkDeviceConfiguration()
    networkDevice.attachment = VZNATNetworkDeviceAttachment()
    vmConfig.networkDevices = [networkDevice]

    // Serial port — bridge to stdin/stdout for RPC
    let serialPort = VZVirtioConsoleDeviceSerialPortConfiguration()
    let inputPipe = Pipe()
    let outputPipe = Pipe()

    serialPort.attachment = VZFileHandleSerialPortAttachment(
        fileHandleForReading: inputPipe.fileHandleForReading,
        fileHandleForWriting: outputPipe.fileHandleForWriting
    )
    vmConfig.serialPorts = [serialPort]

    // Entropy — guest needs randomness
    vmConfig.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]

    // Memory balloon — allows dynamic memory adjustment
    vmConfig.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]

    // Shared directory — host workspace mounted read-only in guest
    // (will be configured per-session via RPC)

    try vmConfig.validate()
    return vmConfig
}

// MARK: - Stdin/Stdout Bridge

/// Bridges this process's stdin/stdout to the VM's serial port.
/// The host Rust code writes RPC requests to our stdin, we forward them
/// to the VM serial port. The VM serial port output goes to our stdout.
class StdioBridge {
    let inputToVM: FileHandle    // We write to this → VM reads from serial
    let outputFromVM: FileHandle // VM writes to serial → we read from this

    init(inputToVM: FileHandle, outputFromVM: FileHandle) {
        self.inputToVM = inputToVM
        self.outputFromVM = outputFromVM
    }

    func start() {
        // Forward host stdin → VM serial input
        DispatchQueue.global(qos: .userInteractive).async {
            let stdin = FileHandle.standardInput
            while true {
                let data = stdin.availableData
                if data.isEmpty { break }
                self.inputToVM.write(data)
            }
        }

        // Forward VM serial output → host stdout
        DispatchQueue.global(qos: .userInteractive).async {
            while true {
                let data = self.outputFromVM.availableData
                if data.isEmpty { break }
                FileHandle.standardOutput.write(data)
            }
        }
    }
}

// MARK: - VM Delegate

class VMDelegate: NSObject, VZVirtualMachineDelegate {
    var onStopped: (() -> Void)?
    var onError: ((Error) -> Void)?

    func virtualMachine(_ vm: VZVirtualMachine, didStopWithError error: Error) {
        fputs("ERROR: VM stopped with error: \(error.localizedDescription)\n", stderr)
        onError?(error)
    }

    func guestDidStop(_ vm: VZVirtualMachine) {
        fputs("INFO: VM guest stopped\n", stderr)
        onStopped?()
    }
}

// MARK: - Create Disk Image

func createDiskImage(path: String, sizeGB: Int) throws {
    let sizeBytes = UInt64(sizeGB) * 1024 * 1024 * 1024
    let fm = FileManager.default

    if fm.fileExists(atPath: path) {
        fputs("INFO: disk image already exists at \(path)\n", stderr)
        return
    }

    // Create a sparse file of the desired size
    fm.createFile(atPath: path, contents: nil)
    let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: path))
    try handle.truncate(atOffset: sizeBytes)
    handle.closeFile()

    fputs("INFO: created \(sizeGB)GB disk image at \(path)\n", stderr)
}

// MARK: - Main

let config = parseArgs()
let bundle = VMBundle(path: config.imagePath)

switch config.action {
case "create":
    do {
        try bundle.ensureExists()
        try createDiskImage(path: bundle.diskPath, sizeGB: config.diskSizeGB)
        print("OK: VM bundle created at \(config.imagePath)")
    } catch {
        fputs("ERROR: \(error.localizedDescription)\n", stderr)
        exit(1)
    }

case "start":
    do {
        let vmConfig = try createVMConfig(config: config, bundle: bundle)
        let vm = VZVirtualMachine(configuration: vmConfig)

        let delegate = VMDelegate()
        let semaphore = DispatchSemaphore(value: 0)

        delegate.onStopped = { semaphore.signal() }
        delegate.onError = { _ in semaphore.signal() }
        vm.delegate = delegate

        // Get the serial port pipes for stdio bridging
        if let serialAttachment = vmConfig.serialPorts.first?.attachment
            as? VZFileHandleSerialPortAttachment {
            // The serial port attachment gives us file handles that connect to the VM.
            // Our stdin/stdout bridge will forward bytes between host and guest.
            guard let inputHandle = serialAttachment.fileHandleForReading,
                  let outputHandle = serialAttachment.fileHandleForWriting else {
                fputs("ERROR: serial port file handles not available\n", stderr)
                exit(1)
            }
            let bridge = StdioBridge(
                inputToVM: inputHandle,
                outputFromVM: outputHandle
            )

            vm.start { result in
                switch result {
                case .success:
                    fputs("INFO: VM started\n", stderr)
                    bridge.start()
                case .failure(let error):
                    fputs("ERROR: VM start failed: \(error.localizedDescription)\n", stderr)
                    exit(1)
                }
            }
        }

        // Block until VM stops
        semaphore.wait()

    } catch {
        fputs("ERROR: \(error.localizedDescription)\n", stderr)
        exit(1)
    }

case "status":
    let fm = FileManager.default
    let exists = fm.fileExists(atPath: bundle.diskPath)
    print(exists ? "OK: bundle exists" : "MISSING: no bundle at \(config.imagePath)")

default:
    fputs("ERROR: unknown action: \(config.action)\n", stderr)
    exit(1)
}
