# A tiny infra-shaped schema, sized so the runnable example stays
# legible but exercises the codegen features Tilley's MVP cares about:
# required + optional fields, schema defaults, string-literal unions
# (lifted to a named Rust enum), nested schemas, and lists of
# nested schemas.

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
