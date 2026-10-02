"""Fill two loopback listen queues to cause TCP timeouts without network changes."""

import json
from pathlib import Path
import signal
import socket
import sys

listeners = []
fillers = []
targets = []
try:
    for family, address in [(socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")]:
        listener = socket.socket(family)
        listener.bind((address, 0))
        listener.listen(1)
        listeners.append(listener)
        filler = socket.socket(family)
        filler.settimeout(1)
        filler.connect(listener.getsockname())
        fillers.append(filler)
        address, port = listener.getsockname()[:2]
        targets.append(("%s:%s" if family == socket.AF_INET else "[%s]:%s") % (address, port))
    config = (
        "tcp_targets = " + json.dumps(targets)
        + '\ndns_name = "localhost"\nhttps_url = "https://' + targets[0] + '/"\n'
    )
    Path(sys.argv[1]).write_text(config)
    print(config, flush=True)
    signal.pause()
except KeyboardInterrupt:
    pass
finally:
    for stream in fillers + listeners:
        stream.close()
