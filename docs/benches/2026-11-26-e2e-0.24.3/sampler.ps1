# Samples the REAL shared stop event and the clipboard sequence number every
# 50 ms until <StopFile> exists. Read-only: opens the event only if it exists
# (never creates or sets it) and never opens the clipboard.
param([Parameter(Mandatory = $true)][string]$StopFile, [Parameter(Mandatory = $true)][string]$Out)
Add-Type -Namespace W -Name U -MemberDefinition '[DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();'
$name = 'Local\ghost-emergency-stop-event'
$rights = [System.Security.AccessControl.EventWaitHandleRights]::Synchronize
$n = 0; $sig = 0; $absent = 0; $seq0 = [W.U]::GetClipboardSequenceNumber(); $seqChanges = 0; $last = $seq0
$t0 = Get-Date
while (-not (Test-Path $StopFile)) {
    $n++
    $h = $null
    if ([System.Threading.EventWaitHandle]::TryOpenExisting($name, $rights, [ref]$h)) {
        if ($h.WaitOne(0)) { $sig++ }
        $h.Close()
    } else { $absent++ }
    $s = [W.U]::GetClipboardSequenceNumber()
    if ($s -ne $last) { $seqChanges++; $last = $s }
    Start-Sleep -Milliseconds 50
}
"started=$($t0.ToString('o')) ended=$((Get-Date).ToString('o'))",
"samples=$n signalled=$sig event_absent=$absent",
"clipboard_seq_first=$seq0 clipboard_seq_last=$last clipboard_seq_changes=$seqChanges" | Set-Content -Encoding ascii $Out
