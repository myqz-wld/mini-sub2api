//go:build nativeparity && liveparity

package integration

import (
	"encoding/json"
	"testing"
)

func newLiveCallerWireGateway(t *testing.T, maximum int32) (nativeGateway, *liveWireRelay) {
	t.Helper()
	if maximum < 1 || maximum > 10 {
		t.Fatal("live caller fixture must fit the relay's 12-request bound including setup")
	}
	relay := newLiveWireRelay(t, false)
	gateway := newLiveSubscriptionGatewayAt(t, maximum, relay.endpoint+"/v1/responses", false)
	return gateway, relay
}

func assertLiveCallerWire(t *testing.T, gateway nativeGateway, relay *liveWireRelay) {
	t.Helper()
	incoming, outgoing := gateway.tap.packets(t), relay.tap.packets(t)
	var packets []nativePacket
	var wires []nativeWire
	for _, packet := range outgoing {
		body := nativePacketJSON(t, packet)
		var value map[string]any
		if json.Unmarshal(body, &value) != nil {
			t.Fatal("live caller capture JSON invalid")
		}
		if value["generate"] == false {
			continue
		}
		packets = append(packets, packet)
		wires = append(wires, nativeWire{method: packet.method, headers: packet.headers, body: body, encodedBody: packet.payload, value: value, connection: packet.connection})
	}
	if len(incoming) != len(wires) || len(wires) == 0 {
		t.Fatal("live caller capture lost or added business inference")
	}
	for i, wire := range wires {
		assertCallerWireContract(t, incoming[i], wire, packets[i])
	}
	t.Logf("live caller ordered wire requests=%d", len(wires))
	relay.report(t)
}
