# Print a print-ready PDF on Windows (issue #756). The job arrives in PDFCRAFT_* environment
# variables, never as arguments: printer names contain spaces and quotes, and the document's
# path must not be visible in the process list while it prints.
#
# The in-box Windows.Data.Pdf renders each sheet at the printer's own resolution and
# System.Drawing.Printing spools it with the driver's settings — copies, collation, duplex and
# colour reach the spooler as the job's, the way `lp -o` sends them on CUPS. No PDF reader is
# needed, and the calling code touches no Win32 API.
#
# PDFCRAFT_PRINT_DRYRUN=1 renders every sheet to a PNG in a temporary folder and prints
# nothing: the tests run the whole rendering path on machines without a printer, CI included.
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Runtime.WindowsRuntime
try {
    $null = [Windows.Data.Pdf.PdfDocument, Windows.Data.Pdf, ContentType = WindowsRuntime]
} catch {
    throw 'this Windows installation cannot render PDFs: the Windows.Data.Pdf component is missing'
}
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]

# Await the WinRT asynchronous APIs from .NET Framework's PowerShell. AsTask is an extension
# method: found by reflecting over the static class — with `.`, an instance call on the type
# object, since `GetMethods` is not static and `::` would not find it.
$type = [System.WindowsRuntimeSystemExtensions]
$opTask = $type.GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
} | Select-Object -First 1
$actTask = $type.GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction'
} | Select-Object -First 1
function AwaitOp($operation, $resultType) {
    $task = $opTask.MakeGenericMethod($resultType).Invoke($null, @($operation))
    $task.Wait()
    $task.Result
}
function AwaitAct($action) {
    $task = $actTask.Invoke($null, @($action))
    $task.Wait()
}

# One page rendered the size it will be drawn (`$drawW` × `$drawH`, hundredths of an inch), at
# `$dpi` dots per inch, aspect-fitted, and handed to `$use` while it lives. GDI+ reads an image
# lazily from its stream, so the stream outlives the use rather than the image being copied
# out of it: a sheet at 600 dpi is tens of megapixels, and the copy cost a quarter second.
function Render-Sheet($page, $drawW, $drawH, $dpi, [scriptblock] $use) {
    $w = $page.Size.Width
    $h = $page.Size.Height
    $scale = [Math]::Min($drawW / 100.0 * 96.0 / $w, $drawH / 100.0 * 96.0 / $h)
    $options = [Windows.Data.Pdf.PdfPageRenderOptions]::new()
    $options.DestinationWidth = [int][Math]::Ceiling($w * $scale * $dpi / 96.0)
    $options.DestinationHeight = [int][Math]::Ceiling($h * $scale * $dpi / 96.0)
    $stream = [System.IO.MemoryStream]::new()
    try {
        $random = [System.IO.WindowsRuntimeStreamExtensions]::AsRandomAccessStream($stream)
        AwaitAct ($page.RenderToStreamAsync($random, $options))
        $stream.Position = 0
        $image = [System.Drawing.Image]::FromStream($stream)
        try { & $use $image } finally { $image.Dispose() }
    } finally {
        $stream.Dispose()
    }
}

$source = AwaitOp ([Windows.Storage.StorageFile]::GetFileFromPathAsync($env:PDFCRAFT_PRINT_FILE)) ([Windows.Storage.StorageFile])
$document = AwaitOp ([Windows.Data.Pdf.PdfDocument]::LoadFromFileAsync($source)) ([Windows.Data.Pdf.PdfDocument])
if ($document.PageCount -lt 1) { throw 'the print-ready PDF has no pages' }

if ($env:PDFCRAFT_PRINT_DRYRUN -eq '1') {
    # Every sheet rendered as the printer would take it, to prove the path without one.
    $folder = Join-Path ([System.IO.Path]::GetTempPath()) ('pdfcraft-dryrun-' + $PID)
    $null = New-Item -ItemType Directory -Force $folder
    for ($i = 0; $i -lt $document.PageCount; $i++) {
        $page = $document.GetPage($i)
        try {
            $file = Join-Path $folder "sheet-$i.png"
            Render-Sheet $page 750 1000 150 { param($image) $image.Save($file, [System.Drawing.Imaging.ImageFormat]::Png) }
        } finally {
            $page.Dispose()
        }
    }
    Write-Output ('rendered ' + $document.PageCount + ' sheet(s)')
    return
}

$printer = [System.Drawing.Printing.PrintDocument]::new()
$printer.DocumentName = $env:PDFCRAFT_PRINT_TITLE
$settings = $printer.PrinterSettings
if ($env:PDFCRAFT_PRINT_PRINTER) {
    try {
        $settings.PrinterName = $env:PDFCRAFT_PRINT_PRINTER
    } catch {
        throw ('the printer is not available: ' + $env:PDFCRAFT_PRINT_PRINTER)
    }
}
$settings.Copies = [Math]::Min(32767, [Math]::Max(1, [int]$env:PDFCRAFT_PRINT_COPIES))
$settings.Collate = ($env:PDFCRAFT_PRINT_COLLATE -eq '1')
$settings.Duplex = switch ($env:PDFCRAFT_PRINT_DUPLEX) {
    'long-edge' { [System.Drawing.Printing.Duplex]::Horizontal }
    'short-edge' { [System.Drawing.Printing.Duplex]::Vertical }
    default { [System.Drawing.Printing.Duplex]::Simplex }
}
if ($env:PDFCRAFT_PRINT_GRAYSCALE -eq '1') { $printer.DefaultPageSettings.Color = $false }

# The printer's papers, asked of the driver once: every read of PaperSizes asks it again.
$papers = @($settings.PaperSizes)

# The printer's paper the size of a sheet (hundredths of an inch, either way round), within a
# millimetre or so; $null when the printer has none, and the sheet is fitted to its paper.
function Find-Paper($wide, $high) {
    foreach ($paper in $papers) {
        foreach ($pair in @(@($paper.Width, $paper.Height), @($paper.Height, $paper.Width))) {
            if ([Math]::Abs($pair[0] - $wide) -le 5 -and [Math]::Abs($pair[1] - $high) -le 5) { return $paper }
        }
    }
    $null
}

# Each sheet on the paper it was laid out for, turned to its orientation: the sheets are already
# the size they print at (the dialog's paper), so the printer's default paper must not stand in.
$script:query = 0
$printer.add_QueryPageSettings({
    param($sender, $e)
    if ($script:query -ge $document.PageCount) { return }
    $page = $document.GetPage($script:query)
    try {
        $wide = $page.Size.Width / 96.0 * 100.0
        $high = $page.Size.Height / 96.0 * 100.0
        $paper = Find-Paper $wide $high
        if ($paper) { $e.PageSettings.PaperSize = $paper }
        $e.PageSettings.Landscape = ($wide -gt $high)
    } finally {
        $page.Dispose()
    }
    $script:query++
})

# `progress SENT SHEETS` on a line of its own, at once: PdfCraft shows how far the job has got
# while the script still runs. Straight to the console, since output from inside the driver's
# callbacks would otherwise wait for the script to end.
function Send-Progress($sent) {
    [Console]::Out.WriteLine('progress ' + $sent + ' ' + $document.PageCount)
    [Console]::Out.Flush()
}

# One sheet at a time: its page is rendered when the driver asks for it, so a long document
# holds only one page's bitmap in memory.
$script:sheets = 0
$printer.add_PrintPage({
    param($sender, $e)
    if ($script:sheets -ge $document.PageCount) { $e.HasMorePages = $false; return }
    $page = $document.GetPage($script:sheets)
    try {
        # The whole sheet of paper, in hundredths of an inch. Drawing starts at the printable
        # area's corner, so the paper's own corner is the hard margin back from it. Not
        # MarginBounds: those are the inch-wide margins of a document laid out by the printer,
        # and the sheet carries its own.
        $area = $e.PageBounds
        $left = -$e.PageSettings.HardMarginX
        $top = -$e.PageSettings.HardMarginY
        $w = $page.Size.Width
        $h = $page.Size.Height
        # 1 when the paper matched the sheet: the sheet prints at its own size.
        $scale = [Math]::Min($area.Width / 100.0 * 96.0 / $w, $area.Height / 100.0 * 96.0 / $h)
        $dpi = [Math]::Min(600, [Math]::Max(72, $e.Graphics.DpiX))
        $drawW = $w * $scale / 96.0 * 100.0
        $drawH = $h * $scale / 96.0 * 100.0
        $box = [System.Drawing.RectangleF]::new($left + ($area.Width - $drawW) / 2.0, $top + ($area.Height - $drawH) / 2.0, $drawW, $drawH)
        # The sheet is rendered at the printer's own resolution (up to 600 dpi), a printer dot
        # per pixel: copied as it is, not resampled again.
        $g = $e.Graphics
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
        $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
        Render-Sheet $page $area.Width $area.Height $dpi { param($image) $g.DrawImage($image, $box) }
    } finally {
        $page.Dispose()
    }
    $script:sheets++
    Send-Progress $script:sheets
    $e.HasMorePages = ($script:sheets -lt $document.PageCount)
})
Send-Progress 0
$printer.Print()
Write-Output ('spooled ' + $document.PageCount + ' sheet(s)')
