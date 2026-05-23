# Diagnostic: connect to the ipod-sync daemon pipe and dump every event.
# Usage: pwsh scripts/probe-daemon.ps1
# Press Ctrl-C to exit.

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'ipod-sync',
    [System.IO.Pipes.PipeDirection]::InOut,
    [System.IO.Pipes.PipeOptions]::Asynchronous)
$pipe.Connect(3000)
Write-Host "Connected to \\.\pipe\ipod-sync" -ForegroundColor Green

# Send GetStatus right away so we see what the daemon currently thinks.
$writer = New-Object System.IO.StreamWriter($pipe, [System.Text.Encoding]::UTF8, 4096, $true)
$writer.WriteLine('{"type":"get_status"}')
$writer.Flush()
Write-Host "Sent get_status" -ForegroundColor Cyan

$reader = New-Object System.IO.StreamReader($pipe, [System.Text.Encoding]::UTF8, $false, 4096, $true)
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { Write-Host "[pipe closed]" -ForegroundColor Red; break }
    $ts = (Get-Date).ToString('HH:mm:ss.fff')
    Write-Host "[$ts] $line" -ForegroundColor White
}
