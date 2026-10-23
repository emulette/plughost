//! Render through an application and helper built only from packaged crates.
use std::path::PathBuf;

use plughost::{
    Chain, Events, Layout, PluginFormat, RenderOptions, Scanner, SpawnOptions, StatePurpose,
    TailPolicy, render,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let helper = PathBuf::from(args.next().unwrap());
    let fixtures = PathBuf::from(args.next().unwrap());
    let cache = PathBuf::from(args.next().unwrap());
    for (format, extension) in [(PluginFormat::Vst3, "vst3"), (PluginFormat::Clap, "clap")] {
        let bundle = fixtures.join(format!("plughost-test-routing.{extension}"));
        let scanner = Scanner::new(&helper, &cache);
        let scanned = scanner.retry(&bundle)?;
        let plugins = scanned.plugins();
        let [(reference, class)] = plugins.as_slice() else {
            panic!("packaged scanner did not find the routing fixture");
        };
        assert_eq!(class.id.format(), format);
        let mut chain = Chain::spawn(
            &helper,
            &[reference.clone().into()],
            &SpawnOptions::default(),
        )?;
        let config = chain.main_bus_config(48_000.0, 64, Layout::Stereo, &[Layout::Stereo])?;
        chain.prepare_audio(&config)?;
        chain.set_parameter(0, 0, 0.5)?;
        let state = chain.save_state(0, StatePurpose::Project)?;
        let input: Vec<f32> = (0..257).map(|i| (i % 8) as f32 / 8.0).collect();
        for gain in [0.5, 0.25, 0.5] {
            if gain == 0.25 {
                chain.set_parameter(0, 0, gain)?;
            } else {
                chain.restore_state(0, &state, StatePurpose::Project)?;
            }
            chain.reset()?;
            let rendered = render(
                &mut chain,
                &[&input, &input],
                input.len(),
                &Events::new(),
                &RenderOptions::new(TailPolicy::Reported, 0.0),
            )?;
            let expected: Vec<f32> = input.iter().map(|v| *v * gain as f32).collect();
            assert_eq!(
                rendered.channels,
                [expected.clone(), expected],
                "{format:?}"
            );
        }
    }
    Ok(())
}
