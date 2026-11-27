<#
.SYNOPSIS
  Runs the title with the switch set measured on an RTX 3060 12 GB / Ryzen 9 5900X / 32 GB RAM.

.EXAMPLE
  scripts\play-rtx3060.ps1 -Game "D:\Games\MyDump" -Victory "C:\vt\release\victory.exe"

.NOTES
  Build first:  cargo build --locked --release -p victory
  You need your own dump of the title; this repository contains no game files.
#>
param(
    [Parameter(Mandatory = $true)][string]$Game,            # folder with game_scarlett_master.exe
    [string]$Victory = "target\release\victory.exe",
    [string]$DataDir = "",                                   # VICTORY_DATA_DIR, if you set one
    [ValidateSet("SeriesS", "SeriesX")][string]$Console = "SeriesS",
    [switch]$Experimental                                    # also the switches merged but not yet verified
)
$ErrorActionPreference = "Stop"
$exe = Join-Path $Game "game_scarlett_master.exe"
if (-not (Test-Path $exe)) { throw "No game_scarlett_master.exe in $Game" }
if (-not (Test-Path $Victory)) { throw "No victory.exe at $Victory (build it first)" }

Get-ChildItem Env:VICTORY_* | Where-Object { $_.Name -notin 'VICTORY_CONFIG', 'VICTORY_GAME_DIR', 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR' } |
    ForEach-Object { Remove-Item "Env:$($_.Name)" }

$set = [ordered]@{
    # avatarmanz's measured base set
    VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'
    VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'
    VICTORY_INDIRECT_BATCH = '2'; VICTORY_CARD_AWAKE = '1'; VICTORY_SHOW_AFTER = '1'; VICTORY_AMBIENT = '0.8'
    VICTORY_BVH_NO_HIT = '1'; VICTORY_PIPELINE_BOOK = '1'; VICTORY_D3D12_ADAPTER = 'NVIDIA'
    VICTORY_THIN = '1'; VICTORY_JOIN_PARTS = '8'; VICTORY_UPLOAD_AHEAD = '1'
    # renderer ports 1, 2, 6 (port 5 needs a heap NVIDIA cannot create)
    VICTORY_PORT_CONSTANTS = '1'; VICTORY_PORT_DESCRIPTORS = '1'; VICTORY_PORT_GUARD = '1'
    # title memory read from VRAM, not over PCIe (Resizable BAR off)
    VICTORY_CARD_WINDOWS_MB = '2304'; VICTORY_CARD_BUFFERS_MIN_KB = '64'; VICTORY_CARD_BUFFERS_MB = '1024'
    # buffers placed in heaps made resident once; change tracking by list
    VICTORY_SHARED_HEAPS = '16'; VICTORY_CARD_HEAPS = '64'; VICTORY_CARD_DIRTY_LIST = '1'; VICTORY_STRUCT_IN_PLACE = '1'
    # threads and memory
    VICTORY_THIN_THREADS = '8'; VICTORY_PIPELINE_WORKERS = '4'; VICTORY_WINDOW_KB = '256'; VICTORY_WINDOW_TAILS = '1'
    VICTORY_ALLOC_TRIM = '1024'; VICTORY_RESIDENCY = '1'; VICTORY_INPLACE = '1'; VICTORY_MEMORY_MAP = '0'
    VICTORY_BCPACK_CACHE_MB = '0'; VICTORY_ALIAS_SAME = '1'; VICTORY_EXACT_LATELY = '8192'
    # picture: the title's own upscaler (FSR4's fallback dots hair and shadow edges)
    VICTORY_UPSCALER = '6'
    # prelude uploads (flicker fix) cost ~3 ms per 1k draws: off unless -Experimental
    VICTORY_PRELUDE_UPLOADS = '0'
    # exact paired wave64 form: all 27 wave64-only shaders translate (run q16: 0 pipelines refused)
    VICTORY_WAVE_PAIRS = '1'
}
if ($Experimental) {
    # merged, waiting for verification (see docs/TRACKER.md)
    $set.VICTORY_WATCH_RESERVATION = '1'; $set.VICTORY_LOOKUP_CACHE = '1'; $set.VICTORY_APU_ASYNC = '1'
    $set.VICTORY_EARLY_TABLES = '1'; $set.VICTORY_INDIRECT_REST = '0'; $set.VICTORY_LEVELS_ON_DEMAND = '1'
    $set.VICTORY_PRELUDE_UPLOADS = '1'
}
foreach ($k in $set.Keys) { Set-Item "Env:$k" $set[$k] }
if ($DataDir) { $env:VICTORY_DATA_DIR = $DataDir }

# A full telemetry cache once hung the title's load; moving it aside is harmless.
foreach ($d in @($DataDir, (Join-Path (Split-Path $Victory -Parent) "data"))) {
    if ($d -and (Test-Path $d)) {
        Get-ChildItem $d -Recurse -Filter telemetrycache.dat -ErrorAction SilentlyContinue |
            ForEach-Object { Move-Item $_.FullName "$($_.FullName).prev" -Force }
    }
}

$mode = if ($Console -eq "SeriesS") { "series-s" } else { "series-x" }
Write-Host "Starting with $($set.Count) switches, console $mode" -ForegroundColor Cyan
& $Victory run "$exe" --execute --console $mode
