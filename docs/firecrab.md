# Firecrab GUI transport

microbox includes both ends of the transport that Firecrab itself does not
provide:

- `microbox agent` runs inside the guest, owns Xvfb and the GUI application,
captures RGB frames, and accepts input events.
- `microbox run ... --runtime firecrab` connects to the agent through a
  Firecrab TCP port forward and renders those frames in the terminal.

The protocol is length-prefixed, bounded to 64 MiB per message, validates every
frame through the normal frame allocation limits, and requires the first client
message to authenticate with `MICROBOX_AGENT_TOKEN`. Use a random token and do
not expose the forwarded port outside the host; the protocol is authenticated
but is not encrypted.

The host sends its terminal-derived pixel dimensions to the guest. Initial
connection and every later terminal resize update the guest XRandR screen,
application window, and capture buffer before the matching frame is rendered.
Stale in-flight frames from the previous size are discarded.

## Guest image

The example image builds the current binary and starts `xeyes`:

```sh
docker build -f examples/firecrab-xeyes/Dockerfile -t REGISTRY/microbox-xeyes:VERSION .
docker push REGISTRY/microbox-xeyes:VERSION
```

Import that reference with Firecrab's `POST /api/oci/import`, then create a VM
from the resulting template. Set its environment and port forward as follows:

```json
{
  "env": { "MICROBOX_AGENT_TOKEN": "RANDOM_SECRET" },
  "portForwards": [
    { "hostPort": 15943, "guestPort": 5943, "protocol": "tcp" }
  ]
}
```

Firecrab accepts port forwards during VM creation or through
`PUT /api/vms/{id}/port-forwards`. Start the VM, then connect:

```sh
MICROBOX_AGENT_TOKEN=RANDOM_SECRET \
  microbox run xeyes --runtime firecrab \
  --firecrab-endpoint 127.0.0.1:15943
```

Dropping the client sends a stop message. The agent also exits when the TCP
connection closes, which drops the application and Xvfb process groups.

## Current boundary

The GUI data plane is implemented and tested independently of Firecrab. VM
image import, network selection, VM creation, and deletion remain explicit
Firecrab control-plane operations because they require operator policy choices.
microbox does not silently choose a network or delete an existing VM.
