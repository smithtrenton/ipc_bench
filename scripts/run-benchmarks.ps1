param(
    [string]$OutputDir = $(Join-Path "results" (Get-Date -Format "yyyyMMdd-HHmmss")),
    [int[]]$MessageSizes = @(64, 1024, 4096, 16384, 32704),
    [int]$MessageCount = 1000,
    [int]$WarmupCount = 100,
    [int]$Trials = 3,
    [switch]$SkipPython
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$outputPath = Join-Path $repoRoot $OutputDir
New-Item -ItemType Directory -Force -Path $outputPath | Out-Null

function Resolve-UvCommand {
    Get-Command uv -ErrorAction SilentlyContinue
}

function Invoke-UvPythonCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    if (-not $script:uvCommand) {
        throw "uv is required for Python benchmark execution"
    }

    & $script:uvCommand.Path "run" "--python" "3.14" "python" @Arguments
}

$uvCommand = Resolve-UvCommand
if (-not $SkipPython -and -not $uvCommand) {
    throw "uv is required to run Python benchmarks; install uv or pass -SkipPython."
}
$nativeMethods = @(
    "copy-roundtrip",
    "anon-pipe",
    "named-pipe-byte-sync",
    "named-pipe-message-sync",
    "named-pipe-overlapped",
    "tcp-loopback",
    "shm-events",
    "shm-semaphores",
    "shm-mailbox-spin",
    "shm-mailbox-hybrid",
    "shm-ring-spin",
    "shm-ring-hybrid",
    "af-unix",
    "udp-loopback",
    "mailslot",
    "rpc",
    "alpc"
)
$pythonMethods = @(
    @{ Name = "py-multiprocessing-pipe"; Module = "benchmarks.methods.python.py_multiprocessing_pipe.run" },
    @{ Name = "py-multiprocessing-queue"; Module = "benchmarks.methods.python.py_multiprocessing_queue.run" },
    @{ Name = "py-socket-tcp-loopback"; Module = "benchmarks.methods.python.py_socket_tcp_loopback.run" },
    @{ Name = "py-shared-memory-events"; Module = "benchmarks.methods.python.py_shared_memory_events.run" },
    @{ Name = "py-shared-memory-queue"; Module = "benchmarks.methods.python.py_shared_memory_queue.run" }
)

$startedAt = (Get-Date).ToString("o")
$manifest = New-Object System.Collections.Generic.List[object]
$failures = New-Object System.Collections.Generic.List[object]
$manifestPath = Join-Path $outputPath "manifest.json"
$runStatusPath = Join-Path $outputPath "run-status.json"
$summaryJsonPath = Join-Path $outputPath "summary.json"
$summaryCsvPath = Join-Path $outputPath "summary.csv"

function Write-Manifest {
    $script:manifest | ConvertTo-Json -Depth 6 | Set-Content -Path $script:manifestPath
}

function Write-RunStatus {
    param(
        [string]$Status,
        [string]$ErrorMessage = $null
    )

    $completedCount = 0
    foreach ($entry in $script:manifest) {
        if ($entry.status -eq "completed") {
            $completedCount += 1
        }
    }

    $runState = New-Object PSObject
    $runState | Add-Member -NotePropertyName started_at -NotePropertyValue $script:startedAt
    $runState | Add-Member -NotePropertyName updated_at -NotePropertyValue ((Get-Date).ToString("o"))
    $runState | Add-Member -NotePropertyName status -NotePropertyValue $Status
    $runState | Add-Member -NotePropertyName build_profile -NotePropertyValue "release"
    $runState | Add-Member -NotePropertyName completed -NotePropertyValue $completedCount
    $runState | Add-Member -NotePropertyName failed -NotePropertyValue $script:failures.Count
    $runState | Add-Member -NotePropertyName error -NotePropertyValue $ErrorMessage
    $runState | Add-Member -NotePropertyName failures -NotePropertyValue ([object[]]$script:failures.ToArray())
    $runState | ConvertTo-Json -Depth 8 | Set-Content -Path $script:runStatusPath
}

function Invoke-BenchmarkCommand {
    param(
        [string]$OutputPath,
        [hashtable]$Entry,
        [scriptblock]$Command
    )

    $hadNativePreference = $null -ne (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue)
    if ($hadNativePreference) {
        $previousNativePreference = $PSNativeCommandUseErrorActionPreference
        $PSNativeCommandUseErrorActionPreference = $false
    }
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"

    try {
        $output = & $Command 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
        if ($hadNativePreference) {
            $PSNativeCommandUseErrorActionPreference = $previousNativePreference
        }
    }

    if ($exitCode -eq 0) {
        $output | Set-Content -Path $OutputPath
        return [PSCustomObject]@{
            status = "completed"
            exit_code = 0
            error = $null
        }
    }

    $errorText = (($output | ForEach-Object { "$_" }) -join "`n").Trim()
    if ([string]::IsNullOrWhiteSpace($errorText)) {
        $errorText = "benchmark command failed with exit code $exitCode"
    }

    $failureReport = [ordered]@{
        method = $Entry.method
        language = $Entry.language
        message_size = $Entry.message_size
        message_count = $Entry.message_count
        warmup_count = $Entry.warmup_count
        trials = $Entry.trials
        status = "failed"
        exit_code = $exitCode
        error = $errorText
    }
    $failureReport | ConvertTo-Json -Depth 6 | Set-Content -Path $OutputPath

    return [PSCustomObject]@{
        status = "failed"
        exit_code = $exitCode
        error = $errorText
    }
}

Push-Location $repoRoot
try {
    Write-RunStatus -Status "running"

    cargo build --release --workspace | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    $metadata = [ordered]@{
        generated_at = (Get-Date).ToString("o")
        operating_system = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
        processor = (Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors)
        rustc = (& rustc --version)
        cargo = (& cargo --version)
        python = if ($uvCommand -and -not $SkipPython) { (Invoke-UvPythonCommand -Arguments @("--version") 2>&1) } else { $null }
        build_profile = "release"
        parameters = @{
            output_dir = $OutputDir
            message_sizes = $MessageSizes
            message_count = $MessageCount
            warmup_count = $WarmupCount
            trials = $Trials
        }
    }
    $metadata | ConvertTo-Json -Depth 6 | Set-Content -Path (Join-Path $outputPath "metadata.json")

    foreach ($size in $MessageSizes) {
        foreach ($method in $nativeMethods) {
            $fileName = "{0}-s{1}-native.json" -f $method, $size
            $destination = Join-Path $outputPath $fileName
            Write-Host "Running $method (size=$size)"
            $entry = [ordered]@{
                method = $method
                language = "rust"
                message_size = $size
                output = $fileName
                message_count = $MessageCount
                warmup_count = $WarmupCount
                trials = $Trials
            }
            $result = Invoke-BenchmarkCommand -OutputPath $destination -Entry $entry -Command {
                cargo run --release -q -p $method -- --message-count $MessageCount --message-size $size --warmup-count $WarmupCount --trials $Trials --format json
            }
            $entry.status = $result.status
            $entry.exit_code = $result.exit_code
            if ($result.error) {
                $entry.error = $result.error
                $script:failures.Add([ordered]@{
                    method = $method
                    language = "rust"
                    message_size = $size
                    output = $fileName
                    exit_code = $result.exit_code
                    error = $result.error
                }) | Out-Null
            }
            $script:manifest.Add($entry) | Out-Null
            Write-Manifest
            Write-RunStatus -Status "running"
        }

        if (-not $SkipPython -and $uvCommand) {
            foreach ($method in $pythonMethods) {
                $fileName = "{0}-s{1}-python.json" -f $method.Name, $size
                $destination = Join-Path $outputPath $fileName
                Write-Host "Running $($method.Name) (size=$size)"
                $entry = [ordered]@{
                    method = $method.Name
                    language = "python"
                    message_size = $size
                    output = $fileName
                    message_count = $MessageCount
                    warmup_count = $WarmupCount
                    trials = $Trials
                }
                $result = Invoke-BenchmarkCommand -OutputPath $destination -Entry $entry -Command {
                    Invoke-UvPythonCommand -Arguments @(
                        "-m", $method.Module,
                        "--message-count",
                        $MessageCount,
                        "--message-size",
                        $size,
                        "--warmup-count",
                        $WarmupCount,
                        "--trials",
                        $Trials,
                        "--format",
                        "json"
                    )
                }
                $entry.status = $result.status
                $entry.exit_code = $result.exit_code
                if ($result.error) {
                    $entry.error = $result.error
                    $script:failures.Add([ordered]@{
                        method = $method.Name
                        language = "python"
                        message_size = $size
                        output = $fileName
                        exit_code = $result.exit_code
                        error = $result.error
                    }) | Out-Null
                }
                $script:manifest.Add($entry) | Out-Null
                Write-Manifest
                Write-RunStatus -Status "running"
            }
        }
    }

    $summary = foreach ($entry in $manifest) {
        $reportPath = Join-Path $outputPath $entry.output
        $report = Get-Content -Raw -Path $reportPath | ConvertFrom-Json
        $hasSummary = $report.PSObject.Properties.Name -contains "summary"
        [PSCustomObject]@{
            method = $entry.method
            language = $entry.language
            message_size = $entry.message_size
            message_count = $entry.message_count
            warmup_count = $entry.warmup_count
            trials = $entry.trials
            status = if ($hasSummary) { "completed" } else { $report.status }
            exit_code = if ($hasSummary) { 0 } else { $report.exit_code }
            error = if ($hasSummary) { $null } else { $report.error }
            average_micros = if ($hasSummary) { $report.summary.average_micros } else { $null }
            total_micros = if ($hasSummary) { $report.summary.total_micros } else { $null }
            min_micros = if ($hasSummary) { $report.summary.min_micros } else { $null }
            max_micros = if ($hasSummary) { $report.summary.max_micros } else { $null }
            stddev_micros = if ($hasSummary) { $report.summary.stddev_micros } else { $null }
            message_rate = if ($hasSummary) { $report.summary.message_rate } else { $null }
            child_ready = if ($report.PSObject.Properties.Name -contains "child_ready") { $report.child_ready } else { $null }
            output = $entry.output
        }
    }

    $summary | ConvertTo-Json -Depth 6 | Set-Content -Path $summaryJsonPath
    $summary | Export-Csv -NoTypeInformation -Path $summaryCsvPath

    if ($failures.Count -gt 0) {
        $errorMessage = "$($failures.Count) benchmark runs failed. See run-status.json for details."
        Write-RunStatus -Status "failed" -ErrorMessage $errorMessage
        throw $errorMessage
    }

    Write-RunStatus -Status "completed"
}
catch {
    Write-Manifest
    Write-RunStatus -Status "failed" -ErrorMessage $_.Exception.Message
    throw
}
finally {
    Pop-Location
}
