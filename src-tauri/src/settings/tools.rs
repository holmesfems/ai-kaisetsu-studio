//! 外部ツール（python・ffmpeg）の場所探しと、子プロセス・ファイル書き込みの道具。

use super::Settings;
use std::path::Path;
use std::process::Command;

/// 外部コマンドを実行して標準出力を受け取る。time を過ぎても終わらなければ止めて None
/// （起動時に画面が出る前に呼ぶので、Python がネットワークドライブなどで止まると窓が出ないまま固まっていた）
fn output_within(mut c: Command, time: std::time::Duration) -> Option<(bool, String)> {
    use std::io::Read;
    let mut child = c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some((st.success(), reader.join().unwrap_or_default())),
            Ok(None) if t0.elapsed() < time => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

const PROBE_TIME: std::time::Duration = std::time::Duration::from_secs(10);

/// 本物の python.exe の場所（"python" がランチャーだと、子プロセスがジョブから外れてアプリと一緒に終わらないため）。
pub fn resolve_python(cmd: &str) -> String {
    static CACHE: std::sync::OnceLock<parking_lot::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    let m = CACHE.get_or_init(|| parking_lot::Mutex::new(Default::default()));
    if let Some(v) = m.lock().get(cmd) {
        return v.clone();
    }
    let mut c = hidden(Command::new(cmd));
    c.args(["-c", "import sys;print(sys.executable)"]);
    let out = output_within(c, PROBE_TIME)
        .map(|(_, o)| o.trim().to_string())
        .filter(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or_else(|| cmd.to_string());
    m.lock().insert(cmd.to_string(), out.clone());
    out
}

/// ffmpeg を探す（設定 → PATH → imageio-ffmpeg）。
pub fn find_ffmpeg(s: &Settings) -> Option<String> {
    if !s.ffmpeg.is_empty() && Path::new(&s.ffmpeg).exists() {
        return Some(s.ffmpeg.clone());
    }
    let mut c = hidden(Command::new("where"));
    c.arg("ffmpeg");
    if let Some((true, o)) = output_within(c, PROBE_TIME) {
        if let Some(l) = o.lines().next() {
            return Some(l.trim().to_string());
        }
    }
    let mut c = hidden(Command::new(&s.python));
    c.args(["-c", "import imageio_ffmpeg;print(imageio_ffmpeg.get_ffmpeg_exe())"]);
    let (_, o) = output_within(c, PROBE_TIME)?;
    let p = o.trim().to_string();
    if !p.is_empty() && Path::new(&p).exists() {
        Some(p)
    } else {
        None
    }
}

/// アプリが終わったら（落ちても）子プロセス（演出ワーカー・VoiSona の中継）も一緒に終わるよう、
/// 自分をジョブに入れる。子はジョブを引き継ぐ。残したいもの（Claude のターミナル）は detached() で外に出す。
pub fn kill_children_on_exit() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::JobObjects::*;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        AssignProcessToJobObject(job, GetCurrentProcess());
        // ハンドルは閉じない（アプリが終わるときに閉じられ、子が終わる）
    }
}

/// ジョブの外で起動する（アプリを閉じても残すもの）。
pub fn detached(mut c: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0100_0000 | 0x0000_0010); // CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_CONSOLE
    }
    c
}

/// ファイルを原子的に書く（同じフォルダの一時ファイルに書いてから置き換える）。
/// 書いている途中のファイルを Python・台本エディタが読んで失敗しないように。
/// 置き換えが（読んでいる人がいて）断られたときは少し待ってやり直し、だめなら直接書く
pub fn write_atomic(p: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    let data = data.as_ref();
    let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = p.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    for i in 0..5 {
        match std::fs::rename(&tmp, p) {
            Ok(()) => return Ok(()),
            Err(_) if i < 4 => std::thread::sleep(std::time::Duration::from_millis(30)),
            Err(_) => {}
        }
    }
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(p, data)
}

/// Windows でコンソール窓を出さずに子プロセスを起動する。
pub fn hidden(mut c: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}
