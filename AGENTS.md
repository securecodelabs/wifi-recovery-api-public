# Development requirements

Implement new Rust code; existing private projects are behavior references only. Do not copy their source or history. Do not create a GitHub repository or push until the user authorizes it. Never store test-host credentials or private captures here.

Run formatting, all-target Clippy with warnings denied, the four configured complexity lints, relevant tests, release builds, and applicable security/license checks locally before pushing. Use the same rules in CI. Do not suppress complexity failures or increase thresholds to pass.

No deployment assets. The product is one executable. Releases need a new explicit release request. Remote testing belongs only under /home/wifi-service/wifi-suite and its subdirectories.

No Docker-related files, container builds, image publication or container-based application tests. Run executables directly, with remote runtime dependencies kept under the same home directory.
