# Raise / lower / read the ISOLATED stop events of a private build
# (Local\ghost-emergency-stop-{event,human}-isolated-<Ns>). Never the real ones:
# refuses to run without a namespace.
param(
    [Parameter(Mandatory = $true)][string]$Ns,
    [ValidateSet('stop', 'human', 'both', 'read')][string]$Which = 'read',
    [int]$Up = 1
)
if ($Ns -notmatch '^[A-Za-z0-9-]{1,32}$') { throw "bad namespace" }
$names = @{
    stop  = "Local\ghost-emergency-stop-event-isolated-$Ns"
    human = "Local\ghost-emergency-stop-human-isolated-$Ns"
}
function Open-Ev($name) {
    $created = $false
    New-Object System.Threading.EventWaitHandle($false, [System.Threading.EventResetMode]::ManualReset, $name, [ref]$created)
}
if ($Which -eq 'read') {
    foreach ($k in 'stop', 'human') {
        $e = Open-Ev $names[$k]
        "$k=" + $(if ($e.WaitOne(0)) { 'SIGNALLED' } else { 'clear' })
        $e.Close()
    }
    exit 0
}
$targets = if ($Which -eq 'both') { 'human', 'stop' } else { $Which }
foreach ($k in $targets) {
    $e = Open-Ev $names[$k]
    if ($Up) { [void]$e.Set() } else { [void]$e.Reset() }
    $e.Close()
    "$k " + $(if ($Up) { 'raised' } else { 'lowered' })
}
