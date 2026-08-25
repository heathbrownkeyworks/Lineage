# Signs a file with Azure Artifact Signing (account coldsun-signing,
# profile effigy-public-clean — see metadata.json beside this script).
# Invoked by Tauri's bundle.windows.signCommand with the target file as %1.
#
# Requirements on this machine:
#   - Azure CLI logged in (az login) as the account holding the
#     "Artifact Signing Certificate Profile Signer" role
#   - Artifact Signing Client Tools (winget: Microsoft.Azure.ArtifactSigningClientTools)
#   - Windows SDK signtool (10.0.26100.0 path hardcoded below; Microsoft's
#     documented minimum is "10.0.2261.755" [sic, their version string])
param([Parameter(Mandatory = $true)][string]$File)

# The signing dlib shells out to `az`; make sure it's on PATH even when the
# parent process was started before the Azure CLI was installed.
$azDir = "C:\Program Files\Microsoft SDKs\Azure\CLI2\wbin"
if ($env:PATH -notlike "*$azDir*") { $env:PATH = "$azDir;$env:PATH" }

$signtool = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe"
$dlib = "$env:LOCALAPPDATA\Microsoft\MicrosoftArtifactSigningClientTools\Azure.CodeSigning.Dlib.dll"
$metadata = Join-Path $PSScriptRoot "metadata.json"

# /tr timestamping is mandatory: Artifact Signing certs are valid for only
# three days; the RFC3161 timestamp keeps signatures valid after expiry.
& $signtool sign /v /fd SHA256 /tr "http://timestamp.acs.microsoft.com" /td SHA256 /dlib $dlib /dmdf $metadata $File
exit $LASTEXITCODE
