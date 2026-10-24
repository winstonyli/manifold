//! `gate surface`: the window's drawing surface recovers when its status goes wrong. `SURFACE outdated` and
//! `SURFACE lost` make the next frame see that status (a display change or an eGPU replug would, and neither can be
//! staged here); `EXPORTSTATE`'s `surface_recoveries` counts the reconfigure and the recreate, and a `SCREENSHOT`
//! after each shows the window still draws. No engine is needed. Build first: `cargo build -p manifold`.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, quit, seed, setup};

pub(crate) fn surface(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "surface") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let _t = Section::start("surface");
    guarded("surface", || {
        let profile = work.join("appdata");
        seed(&exe, &profile, "webview2", &[])?;
        let mut inst = Instance::launch(&exe, &profile, &work, "surface.log")?;
        inst.send("WAITIDLE")?;
        for (n, fault) in [(1, "outdated"), (2, "lost")] {
            inst.send(&format!("SURFACE {fault}"))?;
            let state = inst.export_until(&format!("state-{fault}"), |s| s.0["surface_recoveries"] == n)?;
            check(&format!("{fault}: recoveries is {n}"), state.0["surface_recoveries"] == n);
            let shot = inst.screenshot(&format!("after-{fault}"))?;
            check(&format!("{fault}: the window still draws a frame"), shot.exists());
        }
        quit(&mut inst, "surface");
        Ok(())
    });
    finish(&work)
}
