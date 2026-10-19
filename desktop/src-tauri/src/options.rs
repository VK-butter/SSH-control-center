pub fn benchmark_options(args: &[String]) -> Result<Option<(u8, std::path::PathBuf)>, String> {
    let mut cycles = None;
    let mut report = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--benchmark-cycles" => {
                let value = iter.next().ok_or("--benchmark-cycles needs an integer")?;
                let value: u8 = value
                    .parse()
                    .map_err(|_| "benchmark cycles must be 1..10")?;
                if !(1..=10).contains(&value) {
                    return Err("benchmark cycles must be 1..10".into());
                }
                cycles = Some(value);
            }
            "--benchmark-report" => {
                report = Some(std::path::PathBuf::from(
                    iter.next().ok_or("--benchmark-report needs a path")?,
                ));
            }
            _ => {}
        }
    }
    match (cycles, report) {
        (None, None) => Ok(None),
        (Some(cycles), Some(report)) => Ok(Some((cycles, report))),
        _ => Err("benchmark mode requires both --benchmark-cycles and --benchmark-report".into()),
    }
}

//
