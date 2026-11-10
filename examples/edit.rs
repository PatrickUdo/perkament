//! Hulpmiddel: `edit in.pdf` toont de tekstregels van pagina 1;
//! `edit in.pdf <regelnr> <nieuwe tekst> uit.pdf` vervangt een regel.
use perkament::{pdfops, textedit};
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut doc = pdfops::load(&std::fs::read(&args[0])?)?;
    let lines = textedit::text_lines(&doc, 0)?;
    if args.len() < 4 {
        for (i, l) in lines.iter().enumerate() {
            println!("{i:3} [{:6.1},{:6.1}] {:?}", l.quad[0].0, l.quad[0].1, l.text);
        }
        return Ok(());
    }
    let line = &lines[args[1].parse::<usize>()?];
    println!("{:?}", textedit::replace_line(&mut doc, 0, line, &args[2])?);
    std::fs::write(&args[3], pdfops::to_bytes_for_saving(&doc)?)?;
    Ok(())
}
