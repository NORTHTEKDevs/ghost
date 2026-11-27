# Create the ISOLATED stop event with a DACL that grants only SYSTEM, and hold
# it, so a private-build Ghost started now cannot open it (the fail-closed case).
param([Parameter(Mandatory = $true)][string]$Ns, [int]$Seconds = 30)
if ($Ns -notmatch '^[A-Za-z0-9-]{1,32}$') { throw "bad namespace" }
$name = "Local\ghost-emergency-stop-event-isolated-$Ns"
$sec = New-Object System.Security.AccessControl.EventWaitHandleSecurity
$system = New-Object System.Security.Principal.SecurityIdentifier('S-1-5-18')
$rule = New-Object System.Security.AccessControl.EventWaitHandleAccessRule($system, [System.Security.AccessControl.EventWaitHandleRights]::FullControl, [System.Security.AccessControl.AccessControlType]::Allow)
$sec.AddAccessRule($rule)
$created = $false
$e = New-Object System.Threading.EventWaitHandle($false, [System.Threading.EventResetMode]::ManualReset, $name, [ref]$created, $sec)
"squatting $name created=$created"
Start-Sleep -Seconds $Seconds
$e.Close()
