# Firecrab GUI transport

micro-gui includes both ends of the transport that Firecrab itself does not
provide:

- `micro-gui agent` runs inside the guest, owns Xvfb and the GUI application,
  captures RGB frames, and accepts input events.
- `micro-gui run ... --runtime firecrab` connects to the agent through a
  Firecrab TCP port forward and renders those frames in the terminal.

The protocol is length-prefixed, bounded to 64 MiB per message, validates every
frame through the normal frame allocation limits, and requires the first client
message to authenticate with `MICRO_GUI_AGENT_TOKEN`. Use a random token and do
not expose the forwarded port outside the host; the protocol is authenticated
but is not encrypted.

## Guest image

The example image builds the current binary and starts `xeyes`:

```sh
docker build -f examples/firecrab-xeyes/Dockerfile -t REGISTRY/micro-gui-xeyes:VERSION .
docker push REGISTRY/micro-gui-xeyes:VERSION
```

Import that reference with Firecrab's `POST /api/oci/import`, then create a VM
from the resulting template. Set its environment and port forward as follows:

```json
{
  "env": { "MICRO_GUI_AGENT_TOKEN": "RANDOM_SECRET" },
  "portForwards": [
    { "hostPort": 15943, "guestPort": 5943, "protocol": "tcp" }
  ]
}
```

Firecrab accepts port forwards during VM creation or through
`PUT /api/vms/{id}/port-forwards`. Start the VM, then connect:

```sh
MICRO_GUI_AGENT_TOKEN=RANDOM_SECRET \
  micro-gui run xeyes --runtime firecrab \
  --firecrab-endpoint 127.0.0.1:15943
```

Dropping the client sends a stop message. The agent also exits when the TCP
connection closes, which drops the application and Xvfb process groups.

## Current boundary

The GUI data plane is implemented and tested independently of Firecrab. VM
image import, network selection, VM creation, and deletion remain explicit
Firecrab control-plane operations because they require operator policy choices.
micro-gui does not silently choose a network or delete an existing VM.
