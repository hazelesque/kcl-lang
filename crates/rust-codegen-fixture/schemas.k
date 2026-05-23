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
