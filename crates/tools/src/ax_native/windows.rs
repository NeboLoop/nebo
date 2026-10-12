//! Windows backend: UI Automation through PowerShell.
//!
//! Each call spawns `powershell -EncodedCommand`; the script text is built
//! here with the app name, path and value quoted in, and emits the shared
//! JSON-lines format. The persistent daemon in desktop_tool.rs is private to
//! that module, so this spawns directly.
//! ponytail: one process per call (~1–2 s of PowerShell + Add-Type startup);
//! route through DesktopDaemon if that latency matters.

use super::WalkOpts;
use std::time::Duration;

const STARTUP_GRACE: Duration = Duration::from_secs(10);

/// Shared preamble: assemblies, the app's windows, and the walker.
/// `$win` is the requested window; `$pname`/`$pid`/`$wins` describe the app.
const PRELUDE: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$root = [System.Windows.Automation.AutomationElement]::RootElement
$tops = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
$names = @{}
$exact = $null; $partial = $null
foreach ($w in $tops) {
  $p = $w.Current.ProcessId
  if (-not $names.ContainsKey($p)) { try { $names[$p] = (Get-Process -Id $p -ErrorAction Stop).ProcessName } catch { $names[$p] = '' } }
  $t = $w.Current.Name
  if ($names[$p] -ieq $app -or $t -ieq $app) { if ($null -eq $exact) { $exact = $p } }
  elseif ($names[$p] -ilike "*$app*" -or $t -ilike "*$app*") { if ($null -eq $partial) { $partial = $p } }
}
$pid_ = if ($null -ne $exact) { $exact } else { $partial }
if ($null -eq $pid_) { Write-Error "no window belongs to an application named '$app'"; exit 3 }
$pname = $names[$pid_]
$wins = @($tops | Where-Object { $_.Current.ProcessId -eq $pid_ })
if ($window -lt 1 -or $window -gt $wins.Count) { Write-Error "'$pname' has $($wins.Count) window(s); window $window is out of range"; exit 3 }
$win = $wins[$window - 1]
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
function Resolve-Path2($el, $path) {
  if ($path -eq '') { return $el }
  foreach ($i in $path.Split('.')) {
    $c = $walker.GetFirstChild($el); $n = [int]$i
    while ($n -gt 0 -and $null -ne $c) { $c = $walker.GetNextSibling($c); $n-- }
    if ($null -eq $c) { Write-Error "path ${path}: no child $i (walk the tree again; the window changed)"; exit 4 }
    $el = $c
  }
  return $el
}
"#;

const TREE: &str = r#"
$roles = @{
  'ControlType.Button'='AXButton'; 'ControlType.CheckBox'='AXCheckBox'; 'ControlType.RadioButton'='AXRadioButton';
  'ControlType.Edit'='AXTextField'; 'ControlType.Document'='AXTextArea'; 'ControlType.Hyperlink'='AXLink';
  'ControlType.Text'='AXStaticText'; 'ControlType.Image'='AXImage'; 'ControlType.MenuItem'='AXMenuItem';
  'ControlType.Menu'='AXMenu'; 'ControlType.MenuBar'='AXMenuBar'; 'ControlType.ComboBox'='AXComboBox';
  'ControlType.Slider'='AXSlider'; 'ControlType.Spinner'='AXSlider'; 'ControlType.TabItem'='AXTab';
  'ControlType.Tab'='AXTabGroup'; 'ControlType.List'='AXList'; 'ControlType.ListItem'='AXStaticText';
  'ControlType.Table'='AXTable'; 'ControlType.DataGrid'='AXTable'; 'ControlType.Tree'='AXOutline';
  'ControlType.TreeItem'='AXRow'; 'ControlType.Pane'='AXGroup'; 'ControlType.Group'='AXGroup';
  'ControlType.Custom'='AXGroup'; 'ControlType.Window'='AXWindow'; 'ControlType.ToolBar'='AXToolbar';
  'ControlType.StatusBar'='AXGroup'; 'ControlType.ScrollBar'='AXScrollBar'; 'ControlType.TitleBar'='AXGroup';
  'ControlType.Header'='AXGroup'; 'ControlType.HeaderItem'='AXStaticText'; 'ControlType.ProgressBar'='AXProgressIndicator'
}
$press = @('InvokePatternIdentifiers.Pattern','TogglePatternIdentifiers.Pattern','SelectionItemPatternIdentifiers.Pattern','ExpandCollapsePatternIdentifiers.Pattern')
$winRect = $win.Current.BoundingRectangle
Write-Output (@{app=$pname; pid=$pid_; windows=$wins.Count} | ConvertTo-Json -Compress)
$count = 0; $truncated = $false
$stack = New-Object System.Collections.Stack
$kids = @(); $c = $walker.GetFirstChild($win); $i = 0
while ($null -ne $c) { $kids += ,@($c, "$i", 1); $c = $walker.GetNextSibling($c); $i++ }
for ($k = $kids.Count - 1; $k -ge 0; $k--) { $stack.Push($kids[$k]) }
while ($stack.Count -gt 0) {
  if ($sw.ElapsedMilliseconds -gt $timeout -or $count -ge $max) { $truncated = $true; break }
  $el, $path, $d = $stack.Pop()
  try {
    $cur = $el.Current
    $role = $roles[$cur.ControlType.ProgrammaticName]; if ($null -eq $role) { $role = 'AX' + $cur.ControlType.ProgrammaticName.Replace('ControlType.', '') }
    $r = $cur.BoundingRectangle
    $visible = (-not $r.IsEmpty) -and $r.Width -gt 0 -and $r.Height -gt 0 -and $r.IntersectsWith($winRect) -and (-not $cur.IsOffscreen)
    $flatten = ($role -eq 'AXGroup') -and [string]::IsNullOrEmpty($cur.Name)
    if ($visible -and -not $flatten) {
      $pats = @($el.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName })
      $actions = @()
      foreach ($p in $pats) { if ($press -contains $p -and $actions -notcontains 'AXPress') { $actions += 'AXPress' } }
      $value = $null
      if ($pats -contains 'ValuePatternIdentifiers.Pattern') {
        $vp = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        $value = $vp.Current.Value
        if (-not $vp.Current.IsReadOnly) { $actions += 'AXSetValue' }
      }
      $desc = $cur.HelpText; if ([string]::IsNullOrEmpty($desc)) { $desc = $null }
      $node = [ordered]@{ path=$path; role=$role; title=[string]$cur.Name; value=$value; desc=$desc;
        frame=@([int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height); actions=$actions;
        enabled=[bool]$cur.IsEnabled; focused=[bool]$cur.HasKeyboardFocus }
      Write-Output ($node | ConvertTo-Json -Compress -Depth 3)
      $count++
    }
    if ($d -lt $depth) {
      $kids = @(); $c = $walker.GetFirstChild($el); $i = 0
      while ($null -ne $c) { $kids += ,@($c, "$path.$i", $d + 1); $c = $walker.GetNextSibling($c); $i++ }
      for ($k = $kids.Count - 1; $k -ge 0; $k--) { $stack.Push($kids[$k]) }
    }
  } catch { }
}
Write-Output (@{truncated=$truncated; elapsed_ms=[int]$sw.ElapsedMilliseconds} | ConvertTo-Json -Compress)
"#;

const ACT: &str = r#"
$el = Resolve-Path2 $win $path
$pats = @($el.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName })
if ($action -ne 'AXPress') { Write-Error "$action is not supported on Windows; use AXPress or set"; exit 4 }
if ($pats -contains 'InvokePatternIdentifiers.Pattern') { $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }
elseif ($pats -contains 'TogglePatternIdentifiers.Pattern') { $el.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle() }
elseif ($pats -contains 'SelectionItemPatternIdentifiers.Pattern') { $el.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select() }
elseif ($pats -contains 'ExpandCollapsePatternIdentifiers.Pattern') {
  $ec = $el.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
  if ($ec.Current.ExpandCollapseState -eq 'Collapsed') { $ec.Expand() } else { $ec.Collapse() }
}
else { Write-Error "$($el.Current.ControlType.ProgrammaticName) '$($el.Current.Name)' has no press action; click it by coordinate"; exit 4 }
"#;

const SET: &str = r#"
$el = Resolve-Path2 $win $path
$pats = @($el.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName })
if ($pats -notcontains 'ValuePatternIdentifiers.Pattern') { Write-Error "'$($el.Current.Name)' is not editable"; exit 4 }
$vp = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
if ($vp.Current.IsReadOnly) { Write-Error "'$($el.Current.Name)' is read-only"; exit 4 }
$vp.SetValue($value)
"#;

/// `wait` (the helper's command, `ax_native::run`): poll the app's windows
/// until the condition holds or `$timeout` ms pass. `$for` is `text` (an
/// element's name or a field's value contains `$text`), `appears`/`gone`
/// (an element named `$label`), `menu`/`menu-closed` (a menu is open), or
/// `window` (a window, titled containing `$title` when given). Prints
/// `{"elapsed_ms": n}`; a timeout is `wait_timeout: waited …` on stderr.
const WAIT: &str = r#"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$A = [System.Windows.Automation.AutomationElement]; $T = [System.Windows.Automation.TreeScope]; $all = [System.Windows.Automation.Condition]::TrueCondition
function Get-AppWindows {
  $ids = @(Get-Process -Name $app -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
  $tops = @($A::RootElement.FindAll($T::Children, $all))
  if (-not $ids) { $ids = @($tops | Where-Object { $_.Current.Name.IndexOf($app, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | ForEach-Object { $_.Current.ProcessId }) }
  @($tops | Where-Object { $ids -contains $_.Current.ProcessId })
}
function Test-Named($el) {
  $n = [string]$el.Current.Name
  $n -ieq $label -or $n.Replace('&', '').Trim().TrimEnd(':').Trim() -ieq $label.Trim().TrimEnd(':').Trim()
}
function Test-Holds {
  $wins = @(Get-AppWindows)
  switch ($for) {
    'window' { return [bool]($wins | Where-Object { $title -eq '' -or $_.Current.Name.IndexOf($title, [StringComparison]::OrdinalIgnoreCase) -ge 0 }) }
    { $_ -in 'menu', 'menu-closed' } {
      $isItem = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::MenuItem)
      $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker; $open = $false
      foreach ($w in $wins) { foreach ($i in $w.FindAll($T::Descendants, $isItem)) { $p = $walker.GetParent($i); if ($p -and $p.Current.ControlType -ne [System.Windows.Automation.ControlType]::MenuBar) { $open = $true; break } }; if ($open) { break } }
      return $(if ($for -eq 'menu') { $open } else { -not $open })
    }
  }
  foreach ($w in $wins) {
    foreach ($e in $w.FindAll($T::Descendants, $all)) {
      if ($for -eq 'text') {
        if (([string]$e.Current.Name).IndexOf($text, [StringComparison]::OrdinalIgnoreCase) -ge 0) { return $true }
        $vp = $null; if ($e.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$vp) -and ([string]$vp.Current.Value).IndexOf($text, [StringComparison]::OrdinalIgnoreCase) -ge 0) { return $true }
      } elseif (Test-Named $e) { return ($for -eq 'appears') }
    }
  }
  return ($for -eq 'gone')
}
while ($true) {
  if (Test-Holds) { Write-Output (@{ elapsed_ms = [int]$sw.ElapsedMilliseconds } | ConvertTo-Json -Compress); exit 0 }
  if ($sw.ElapsedMilliseconds -ge $timeout) { [Console]::Error.WriteLine("wait_timeout: waited $timeout ms for $what in $app; it did not happen"); exit 1 }
  Start-Sleep -Milliseconds 250
}
"#;

/// The helper's `wait` command: `wait --app A --for KIND [--text T]
/// [--label L] [--title T] --timeout-ms N`.
pub(super) async fn wait_raw(args: &[String], deadline: Duration) -> Result<String, String> {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or("")
    };
    let kind = flag("--for");
    let timeout: u64 = flag("--timeout-ms").parse().unwrap_or(5_000);
    let what = match kind {
        "text" => format!("the text \"{}\"", flag("--text")),
        "appears" => format!("\"{}\" to appear", flag("--label")),
        "gone" => format!("\"{}\" to go", flag("--label")),
        "menu" => "a menu to open".to_string(),
        "menu-closed" => "the menu to close".to_string(),
        "window" if !flag("--title").is_empty() => format!("a window titled \"{}\"", flag("--title")),
        "window" => "a window".to_string(),
        other => return Err(format!("wait: unknown condition '{other}'")),
    };
    let script = format!(
        "$app = {}; $for = {}; $text = {}; $label = {}; $title = {}; $timeout = {timeout}; $what = {}\n{WAIT}",
        ps_quote(flag("--app")),
        ps_quote(kind),
        ps_quote(flag("--text")),
        ps_quote(flag("--label")),
        ps_quote(flag("--title")),
        ps_quote(&what)
    );
    run(&script, deadline + STARTUP_GRACE).await
}

/// Single-quoted PowerShell literal: the only escape is doubling the quote.
/// Windows.Media.Ocr through WinRT: the OS's own recognizer, the user's
/// profile languages. Boxes are the union of each line's word boxes.
const TEXT: &str = r#"
$null = [Windows.Media.Ocr.OcrEngine,Windows.Foundation,ContentType=WindowsRuntime]
$null = [Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder,Windows.Graphics.Imaging,ContentType=WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStream,Windows.Storage.Streams,ContentType=WindowsRuntime]
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, $type) { $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait(); $t.Result }
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync(IMAGE_PATH)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) { [Console]::Error.WriteLine('no OCR language pack installed for the user profile languages'); exit 1 }
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
foreach ($line in $result.Lines) {
  $x1 = [double]::MaxValue; $y1 = [double]::MaxValue; $x2 = 0.0; $y2 = 0.0
  foreach ($w in $line.Words) { $r = $w.BoundingRect; $x1 = [Math]::Min($x1, $r.X); $y1 = [Math]::Min($y1, $r.Y); $x2 = [Math]::Max($x2, $r.X + $r.Width); $y2 = [Math]::Max($y2, $r.Y + $r.Height) }
  Write-Output (@{ text = $line.Text; frame = @([int]$x1, [int]$y1, [int]($x2 - $x1), [int]($y2 - $y1)); confidence = 1.0 } | ConvertTo-Json -Compress)
}
"#;

pub(super) async fn text_raw(image: &std::path::Path) -> Result<String, String> {
    run(&text_script(&image.to_string_lossy()), Duration::from_secs(20) + STARTUP_GRACE).await
}

fn text_script(image: &str) -> String {
    TEXT.replace("IMAGE_PATH", &ps_quote(image))
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A child-index path is digits and dots, nothing else; it is spliced into
/// the script as a literal, so anything else is refused before spawning.
fn valid_path(path: &str) -> bool {
    !path.is_empty() && path.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `-EncodedCommand` takes base64 of the UTF-16LE script.
fn encoded_command(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

fn tree_script(app: &str, opts: &WalkOpts) -> String {
    format!(
        "$app = {}; $window = {}; $depth = {}; $max = {}; $timeout = {}\n{PRELUDE}\n{TREE}",
        ps_quote(app),
        opts.window,
        opts.depth,
        opts.max,
        opts.timeout.as_millis()
    )
}

fn act_script(app: &str, window: usize, path: &str, action: &str) -> String {
    format!(
        "$app = {}; $window = {}; $path = {}; $action = {}\n{PRELUDE}\n{ACT}",
        ps_quote(app),
        window,
        ps_quote(path),
        ps_quote(action)
    )
}

fn set_script(app: &str, window: usize, path: &str, value: &str) -> String {
    format!(
        "$app = {}; $window = {}; $path = {}; $value = {}\n{PRELUDE}\n{SET}",
        ps_quote(app),
        window,
        ps_quote(path),
        ps_quote(value)
    )
}

pub(super) async fn tree_raw(app: &str, opts: &WalkOpts) -> Result<String, String> {
    run(&tree_script(app, opts), opts.timeout + STARTUP_GRACE).await
}

pub(super) async fn act_raw(app: &str, window: usize, path: &str, action: &str, _expect: super::Expect<'_>) -> Result<(), String> {
    if !valid_path(path) {
        return Err(format!("bad path {path:?}: expected child indices like 0.3.2"));
    }
    run(&act_script(app, window, path, action), Duration::from_secs(5) + STARTUP_GRACE).await.map(|_| ())
}

pub(super) async fn set_raw(app: &str, window: usize, path: &str, value: &str, _expect: super::Expect<'_>) -> Result<(), String> {
    if !valid_path(path) {
        return Err(format!("bad path {path:?}: expected child indices like 0.3.2"));
    }
    run(&set_script(app, window, path, value), Duration::from_secs(5) + STARTUP_GRACE).await.map(|_| ())
}

/// Wraps every script: no progress records, and any error is written to
/// stderr as its bare message. Unwrapped, `-EncodedCommand` with stderr
/// redirected serializes both as CLIXML, and the reported error read
/// `#< CLIXML`.
const ERRORS_AS_TEXT: &str = "$ProgressPreference = 'SilentlyContinue'\n\
     $ErrorActionPreference = 'Stop'\n\
     trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }\n";

/// What a failed run is reported as: PowerShell's own error line when there
/// is one, the exit status when there is not. A CLIXML stream (a record the
/// wrap could not keep out) is never the error line.
fn failure_text(code: Option<i32>, stderr: &str) -> String {
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("#< CLIXML") && !l.starts_with("<Objs"))
        .unwrap_or("");
    if first.is_empty() {
        format!("ax helper exited with status {}", code.map_or("unknown".to_string(), |c| c.to_string()))
    } else {
        first.to_string()
    }
}

async fn run(script: &str, timeout: Duration) -> Result<String, String> {
    let mut cmd = command::new::<tokio::process::Command>("powershell", command::Console::Hidden);
    let script = format!("{}{ERRORS_AS_TEXT}{script}", command::POWERSHELL_UTF8);
    cmd.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded_command(&script)])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("ax helper did not finish within {}s", timeout.as_secs()))?
        .map_err(|e| format!("powershell is not available: {e}"))?;
    if !out.status.success() {
        return Err(failure_text(out.status.code(), &String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_script_binds_the_image_path_quoted() {
        let s = text_script("C:\\tmp\\it's.png");
        assert!(s.contains("GetFileFromPathAsync('C:\\tmp\\it''s.png')"), "{s}");
        assert!(!s.contains("IMAGE_PATH"));
    }

    #[test]
    fn ps_quote_doubles_single_quotes_only() {
        assert_eq!(ps_quote("Bob's \"App\" $x"), "'Bob''s \"App\" $x'");
    }

    #[test]
    fn valid_path_is_digits_and_dots() {
        assert!(valid_path("0"));
        assert!(valid_path("0.3.12"));
        for bad in ["", ".", "0.", "a.b", "0..1", "1;rm", "0.-1"] {
            assert!(!valid_path(bad), "{bad:?} accepted");
        }
    }

    #[test]
    fn encoded_command_is_base64_of_utf16le() {
        assert_eq!(encoded_command("hi"), "aABpAA==");
    }

    #[test]
    fn scripts_bind_their_arguments_before_the_prelude() {
        let opts = WalkOpts { window: 2, depth: 4, max: 30, timeout: Duration::from_millis(1500), root: None };
        let s = tree_script("It's Me", &opts);
        assert!(s.starts_with("$app = 'It''s Me'; $window = 2; $depth = 4; $max = 30; $timeout = 1500\n"));
        assert!(s.contains("ControlViewWalker") && s.contains("elapsed_ms"));
        let a = act_script("X", 1, "0.3", "AXPress");
        assert!(a.starts_with("$app = 'X'; $window = 1; $path = '0.3'; $action = 'AXPress'\n"));
        assert!(set_script("X", 1, "0", "v'1").contains("$value = 'v''1'"));
    }

    #[test]
    fn failure_text_prefers_powershells_first_error_line() {
        assert_eq!(failure_text(Some(3), "no window belongs to an application named 'x'\nAt line:1"), "no window belongs to an application named 'x'");
        assert_eq!(failure_text(Some(1), ""), "ax helper exited with status 1");
        assert_eq!(failure_text(Some(1), "#< CLIXML\n<Objs Version=\"1.1.0.1\"></Objs>\nno OCR language pack"), "no OCR language pack");
    }

    /// Every script parses. One that does not never runs at all: the
    /// prelude's `"path $path: …"` made every walk on Windows fail as
    /// "ax helper exited with status 1".
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn every_script_parses() {
        let opts = WalkOpts { window: 1, depth: 4, max: 30, timeout: Duration::from_millis(1500), root: None };
        for (name, script) in [
            ("tree", tree_script("X", &opts)),
            ("act", act_script("X", 1, "0.1", "AXPress")),
            ("set", set_script("X", 1, "0", "v")),
            ("text", text_script("C:\\x.png")),
            ("wait", format!("$app = 'X'; $for = 'text'; $text = 'a'; $label = ''; $title = ''; $timeout = 100; $what = 'x'\n{WAIT}")),
        ] {
            let check = format!(
                "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$e); \
                 if ($e) {{ [Console]::Error.WriteLine($e[0].Message + ' at line ' + $e[0].Extent.StartLineNumber); exit 1 }}",
                ps_quote(&format!("{}{ERRORS_AS_TEXT}{script}", command::POWERSHELL_UTF8))
            );
            if let Err(e) = run(&check, Duration::from_secs(30)).await {
                panic!("the {name} script does not parse: {e}");
            }
        }
    }

    /// An error reaches the caller as its message, not as CLIXML.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn an_error_is_reported_as_its_message() {
        let e = run("Write-Error \"no window belongs to an application named 'x'\"; exit 3", Duration::from_secs(30)).await.unwrap_err();
        assert_eq!(e, "no window belongs to an application named 'x'");
        let e = run("[void][System.IO.File]::ReadAllText('C:\\no\\such\\file.txt')", Duration::from_secs(30)).await.unwrap_err();
        assert!(e.contains("C:\\no\\such\\file.txt") && !e.contains("CLIXML"), "{e}");
    }
}
