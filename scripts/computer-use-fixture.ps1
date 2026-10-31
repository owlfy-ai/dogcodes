param([Parameter(Mandatory=$true)][string]$OutputDirectory, [string]$Instance = 'acceptance', [string]$VisualCode = '4729')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
[System.IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
$form = New-Object System.Windows.Forms.Form
$form.Text = "dog.codes Computer Use Fixture - $Instance"
$form.Size = New-Object System.Drawing.Size(900,650)
$form.StartPosition = 'CenterScreen'
$form.Font = New-Object System.Drawing.Font('Segoe UI',12)
function Label([string]$text,[int]$x,[int]$y) {
    $label=New-Object System.Windows.Forms.Label
    $label.Text=$text; $label.Location=New-Object System.Drawing.Point($x,$y); $label.AutoSize=$true
    $form.Controls.Add($label)
}
function Input([string]$name,[int]$x,[int]$y,[int]$width) {
    $inputBox=New-Object System.Windows.Forms.TextBox
    $inputBox.AccessibleName=$name; $inputBox.Name=$name; $inputBox.Location=New-Object System.Drawing.Point($x,$y); $inputBox.Width=$width
    $form.Controls.Add($inputBox)
    return $inputBox
}
Label 'Project name' 30 25
$project=Input 'Project name' 30 55 700
Label 'Notes' 30 105
$notes=Input 'Notes' 30 135 700
$notes.Multiline=$true; $notes.Height=100
Label 'Read the code from the picture, then enter it below' 30 255
$picture=New-Object System.Windows.Forms.Panel
$picture.Location=New-Object System.Drawing.Point(30,290); $picture.Size=New-Object System.Drawing.Size(240,80)
$picture.BackColor=[System.Drawing.Color]::FromArgb(230,240,250)
$picture.AccessibleName='Visual challenge picture'
$picture.Add_Paint({param($sender,$event) $font=New-Object System.Drawing.Font('Consolas',34,[System.Drawing.FontStyle]::Bold); $event.Graphics.DrawString($VisualCode,$font,[System.Drawing.Brushes]::DarkBlue,12,8); $font.Dispose()})
$form.Controls.Add($picture)
$code=Input 'Verification code' 30 385 240
$save=New-Object System.Windows.Forms.Button
$save.Text='Save report'; $save.AccessibleName='Save report'; $save.Location=New-Object System.Drawing.Point(30,440); $save.Size=New-Object System.Drawing.Size(180,45)
$status=New-Object System.Windows.Forms.Label
$status.Text='Not saved'; $status.AccessibleName='Save status'; $status.Location=New-Object System.Drawing.Point(30,510); $status.Size=New-Object System.Drawing.Size(800,50)
$save.Add_Click({
    if ($code.Text -ne $VisualCode) { $status.Text='Incorrect picture code'; return }
    if (-not $project.Text) { $status.Text='Project name is required'; return }
    $data=@{project=$project.Text;notes=$notes.Text;visualCode=$code.Text;instance=$Instance}
    [System.IO.File]::WriteAllText((Join-Path $OutputDirectory 'result.json'),($data | ConvertTo-Json),[System.Text.UTF8Encoding]::new($false))
    $status.Text='Report saved successfully'
})
$form.Controls.Add($save); $form.Controls.Add($status)
try { [System.Windows.Forms.Application]::Run($form) } finally { $form.Dispose() }
