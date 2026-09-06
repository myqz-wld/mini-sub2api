//go:build nativeparity && liveparity && scaffoldparity

package integration

import (
	"strings"
	"testing"
)

func TestLiveSubscriptionMemoryOpenCode(t *testing.T) {
	gateway := newLiveSubscriptionGatewayBounded(t, 10)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			label, steps := liveMemoryScenario(t)
			client := startOpenCode(t, gateway.server.URL, gateway.secret, model)
			created := client.call(t, "/session", map[string]any{"title": "Synthetic memory session"})
			session, ok := created["id"].(string)
			if !ok || session == "" {
				t.Fatal("live OpenCode memory session absent")
			}
			for _, step := range steps {
				result := client.turn(t, session, step.prompt+" Do not use tools or Markdown.")
				info, _ := result["info"].(map[string]any)
				if info["error"] != nil {
					t.Fatal("live OpenCode memory inference failed")
				}
				parts, _ := result["parts"].([]any)
				var text strings.Builder
				for _, raw := range parts {
					part, _ := raw.(map[string]any)
					if part["type"] == "text" {
						value, _ := part["text"].(string)
						text.WriteString(value)
					}
				}
				assertLiveMemory(t, text.String(), label, step.count)
			}
		})
	}
}
