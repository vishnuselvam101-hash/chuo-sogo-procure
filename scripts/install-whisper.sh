#!/usr/bin/env bash
# Optional, run once as root: builds whisper.cpp v1.9.5 (speech-to-text for the listing assistant) into /opt/whisper.cpp
# and copies the multilingual "small" model next to it, readable by the procure service.
set -euo pipefail
[ -d /opt/whisper.cpp ] || git clone --depth 1 --branch v1.9.5 https://github.com/ggml-org/whisper.cpp /opt/whisper.cpp
cmake -S /opt/whisper.cpp -B /opt/whisper.cpp/build -DCMAKE_BUILD_TYPE=Release
cmake --build /opt/whisper.cpp/build -j 6 --target whisper-cli
install -d -m 0755 /opt/whisper.cpp/models
install -m 0644 "${WHISPER_MODEL:-/root/vnx-studio/models/ggml-small.bin}" /opt/whisper.cpp/models/ggml-small.bin
/opt/whisper.cpp/build/bin/whisper-cli --help >/dev/null && echo "whisper-cli OK"
