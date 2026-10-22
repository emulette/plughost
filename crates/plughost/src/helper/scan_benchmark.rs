use super::*;
use plughost_core::PluginInfo;

#[test]
#[ignore = "needs the helper and native delay fixture"]
#[allow(clippy::print_stdout)]
fn warm_scan_benchmark() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let helper = target.join(if cfg!(target_os = "macos") {
        "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
    } else {
        "helper/plughost-helper.exe"
    });
    let requested_bundle = std::env::var_os("PLUGHOST_SCAN_BUNDLE");
    let bundle = requested_bundle
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| target.join("test-plugins/plughost-test-delay.clap"));
    let scan = || -> Vec<PluginInfo> {
        let mut helper = Helper::spawn(&helper, Mode::Scan).unwrap();
        let timeout = Duration::from_secs(10);
        helper
            .send(
                Request::Scan {
                    bundle: bundle.clone(),
                    host: plughost_core::HostIdentity::default(),
                    interactive: false,
                },
                timeout,
            )
            .unwrap();
        assert_eq!(helper.receive(timeout).unwrap(), Response::ModuleLoaded);
        let mut classes = Vec::new();
        loop {
            match helper.receive(timeout).unwrap() {
                Response::Class(info) => classes.push(info),
                Response::Done => break,
                response => panic!("unexpected scan response: {response:?}"),
            }
        }
        assert!(!classes.is_empty());
        classes
    };
    let expected = scan();
    if requested_bundle.is_none() {
        assert_eq!(expected.len(), 1);
        assert_eq!(
            expected[0].class_id,
            "com.studio.plughost.test-delay.79000001"
        );
    }
    let start = Instant::now();
    for _ in 0..30 {
        assert_eq!(scan(), expected);
    }
    println!("30 warm scans: {:?}", start.elapsed());
    println!("classes: {}", serde_json::to_string(&expected).unwrap());
}
