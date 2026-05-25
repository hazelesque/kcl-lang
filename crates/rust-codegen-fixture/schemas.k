schema Disk:
    size_gb: int
    label?: str
    storage_class: "ssd" | "hdd" | "nvme" = "ssd"

schema Vm:
    name: str
    memory_mb: int = 1024
    cores: int = 2
    notes?: str
    disks: [Disk]
    state: "running" | "stopped" | "paused" = "stopped"

# Phase 4B: tagged-enum codegen lifted from a single schema with a
# discriminator field. Mirrors Tilley's TestAssertion shape.
# @rust: tagged_enum(discriminator = "kind")
schema Action:
    kind: "noop" | "log" | "exec"
    note?: str  # @rust: shared
    message?: str  # @rust: variant("log")
    cmd?: str  # @rust: variant("exec")
    timeout_s?: int = 30  # @rust: variant("exec")
