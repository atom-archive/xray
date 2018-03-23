 #!/bin/bash
(set -o igncr) 2>/dev/null && set -o igncr; # For Cygwin/MSys2 on Windows compatibility

cargo build
if [ $? != 0 ]; then
    exit 1
fi
cargo test
if [ $? != 0 ]; then
    exit 1
fi

cd xray_electron
npm install
if [ $? != 0 ]; then
    exit 1
fi
npm test
if [ $? != 0 ]; then
    exit 1
fi
cd ../