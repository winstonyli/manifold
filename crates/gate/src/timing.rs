//! `TIMING` lines joined with the CPU load around each step (was scripts/gate-timing.sh): `gate all` collects each
//! stage's `TIMING name secs start` lines as `TIMING tag/name secs start cpu_start=% cpu_mean=%`, then lists the slowest.
//! `cpu_start` is the last sample taken at or before the step began, `cpu_mean` the mean of the samples taken during
//! it; `-` where there is none. Samples are the lines of the sampler's file, `<unix seconds> <total cpu %>`.

/// One CPU sample: when it was taken and the reading as the sampler wrote it.
pub struct Sample {
    pub at: f64,
    pub cpu: String,
}

/// Parses the sampler's file (`<unix seconds> <cpu %>` per line); lines that do not parse are skipped.
pub fn parse_samples(text: &str) -> Vec<Sample> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some(Sample { at: f.next()?.parse().ok()?, cpu: f.next()?.to_string() })
        })
        .collect()
}

/// The `TIMING` lines of `run_log`, tagged `tag/` and joined with `samples`.
pub fn collect(tag: &str, run_log: &str, samples: &[Sample]) -> Vec<String> {
    run_log
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let ["TIMING", name, secs, start] = f[..] else {
                return None;
            };
            let (s, d) = (start.parse::<f64>().ok()?, secs.parse::<f64>().ok()?);
            let start_cpu = samples.iter().rfind(|x| x.at <= s).map_or("-", |x| x.cpu.as_str());
            let during: Vec<f64> =
                samples.iter().filter(|x| x.at >= s && x.at <= s + d).filter_map(|x| x.cpu.parse().ok()).collect();
            let mean = if during.is_empty() {
                "-".to_string()
            } else {
                format!("{:.1}", during.iter().sum::<f64>() / during.len() as f64)
            };
            Some(format!("TIMING {tag}/{name} {secs} {start} cpu_start={start_cpu} cpu_mean={mean}"))
        })
        .collect()
}

/// The `n` slowest `TIMING` lines of `log`, slowest first: seconds, name, cpu at start, mean cpu.
pub fn top(log: &str, n: usize) -> Vec<String> {
    let mut rows: Vec<(f64, Vec<&str>)> = log
        .lines()
        .filter(|l| l.starts_with("TIMING "))
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            Some((f.get(2)?.parse().ok()?, f))
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    rows.iter()
        .take(n)
        .map(|(_, f)| format!("{:>8}s  {:<36} {} {}", f[2], f[1], f.get(4).unwrap_or(&""), f.get(5).unwrap_or(&"")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "TIMING setup 12.345 100\nTIMING s1 30.000 112\nnoise\nTIMING s2 3.5 142\n";
    const CPU: &str = "99 5.0\n101 10.0\n105 20.0\n130 40.0\n143 90.0\n150 80.0\n";

    #[test]
    fn collect_joins_each_step_with_the_cpu_samples() {
        assert_eq!(
            collect("g", RUN, &parse_samples(CPU)),
            [
                "TIMING g/setup 12.345 100 cpu_start=5.0 cpu_mean=15.0",
                "TIMING g/s1 30.000 112 cpu_start=20.0 cpu_mean=40.0",
                "TIMING g/s2 3.5 142 cpu_start=40.0 cpu_mean=90.0",
            ]
        );
        assert_eq!(collect("g", RUN, &[])[0], "TIMING g/setup 12.345 100 cpu_start=- cpu_mean=-");
    }

    #[test]
    fn top_lists_the_slowest_first() {
        let all = collect("g", RUN, &parse_samples(CPU)).join("\n");
        let names: Vec<String> =
            top(&all, 2).iter().map(|l| l.split_whitespace().nth(1).unwrap().to_string()).collect();
        assert_eq!(names, ["g/s1", "g/setup"]);
    }
}
