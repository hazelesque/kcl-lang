schema Disk:
    size_gb: int
    label?: str

schema Vm:
    name: str
    memory_mb: int = 1024
    cores: int = 2
    notes?: str
    disks: [Disk]
