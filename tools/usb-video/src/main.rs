use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut output: Option<PathBuf> = None;
    let mut seconds = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" if output.is_none() => {
                output = Some(args.next().ok_or("--output needs a path")?.into())
            }
            "--seconds" if seconds.is_none() => {
                seconds = Some(
                    args.next()
                        .ok_or("--seconds needs a duration")?
                        .parse::<u64>()?,
                )
            }
            _ => return Err("usage: magik-usb-video [--output PATH] [--seconds 1..60]".into()),
        }
    }
    let artifact = match seconds {
        Some(seconds) => magik_usb_video::capture::execute_movie(output.as_deref(), seconds)?,
        None => magik_usb_video::capture::execute(output.as_deref())?,
    };
    println!("{}", serde_json::to_string(&artifact)?);
    Ok(())
}
