package broker

import (
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/brig-sh/brig/internal/policy"
)

type localUpstream func(*http.Request) (*http.Response, error)

func (f localUpstream) RoundTrip(req *http.Request) (*http.Response, error) { return f(req) }

func TestTugboatProjectFetchGrant(t *testing.T) {
	const credential = "synthetic-credential-for-scratch-only"
	data, err := os.ReadFile(os.Getenv("TUGBOAT_FETCH_GRANT_FIXTURE"))
	if err != nil {
		t.Fatal(err)
	}
	document, err := policy.Parse(data)
	if err != nil {
		t.Fatal(err)
	}
	expanded, err := policy.Routes([]policy.Policy{document})
	if err != nil || len(expanded) != 1 {
		t.Fatalf("fixture routes: count=%d error=%v", len(expanded), err)
	}
	routes := []Route{{Policy: document.Metadata.Name, Route: expanded[0]}}
	project := Sandbox{ID: "brig-a", Profile: "preflight", Routes: routes}
	other := Sandbox{ID: "brig-b", Profile: "preflight"}
	creds := newFakeCreds()
	creds.set("preflight-synthetic", []byte(credential))
	audit := &syncBuffer{}
	forwardCount := 0
	handler, err := New(Config{
		HostKey:     testHostKey,
		Credentials: creds,
		Audit:       slog.New(slog.NewTextHandler(audit, nil)),
		Sandboxes:   []Sandbox{project, other},
		Transport: localUpstream(func(req *http.Request) (*http.Response, error) {
			user, password, hasBasic := req.BasicAuth()
			if !hasBasic || user != "x-access-token" || password != credential {
				return nil, fmt.Errorf("wrong synthetic upstream auth")
			}
			forwardCount++
			return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader("scratch-upstream-response")), Request: req}, nil
		}),
	})
	if err != nil {
		t.Fatal(err)
	}
	token, err := handler.Mint(project.ID, "a", "github", DefaultTokenTTL)
	if err != nil {
		t.Fatal(err)
	}
	send := func(name, method, path, access string, expected int) {
		t.Helper()
		req := httptest.NewRequest(method, "https://brig.internal"+path, strings.NewReader(""))
		if access != "" {
			req.SetBasicAuth("x-access-token", access)
		}
		response := httptest.NewRecorder()
		handler.ServeHTTP(response, req)
		if response.Code != expected {
			t.Fatalf("%s status %d, expected %d", name, response.Code, expected)
		}
		if strings.Contains(response.Body.String(), credential) || strings.Contains(response.Body.String(), token) {
			t.Fatalf("%s exposed a credential", name)
		}
		t.Logf("%s: %d", name, response.Code)
	}
	refuseMint := func(sandbox, session string) {
		t.Helper()
		if issued, err := handler.Mint(sandbox, session, "github", DefaultTokenTTL); err == nil || issued != "" {
			t.Fatal("the broker issued a token without the grant")
		}
	}
	const fetchDiscovery = "/brig-a/r/github/demo/private.git/info/refs?service=git-upload-pack"
	const fetchUpload = "/brig-a/r/github/demo/private.git/git-upload-pack"
	const otherFetch = "/brig-b/r/github/demo/private.git/info/refs?service=git-upload-pack"
	send("fetch discovery", "GET", fetchDiscovery, token, 200)
	send("fetch upload pack", "POST", fetchUpload, token, 200)
	send("push denied", "POST", "/brig-a/r/github/demo/private.git/git-receive-pack", token, 403)
	send("second repository denied", "GET", "/brig-a/r/github/demo/second.git/info/refs?service=git-upload-pack", token, 403)
	refuseMint(other.ID, "b")
	send("other project has no grant", "GET", otherFetch, "", 404)
	send("other project cannot use the first token", "GET", otherFetch, token, 404)
	handler.RemoveSandbox(project.ID)
	refuseMint(project.ID, "a")
	send("previous token denied after revocation", "POST", fetchUpload, token, 404)
	if err := handler.AddSandbox(Sandbox{ID: project.ID, Profile: project.Profile}); err != nil {
		t.Fatal(err)
	}
	refuseMint(project.ID, "a")
	send("fresh fetch denied after re-registration without the grant", "GET", fetchDiscovery, "", 404)
	send("previous token denied after re-registration without the grant", "POST", fetchUpload, token, 404)
	if err := handler.AddSandbox(project); err != nil {
		t.Fatal(err)
	}
	send("previous token denied after a new grant", "POST", fetchUpload, token, 401)
	if forwardCount != 2 {
		t.Fatalf("unexpected upstream count %d", forwardCount)
	}
	if strings.Contains(audit.String(), credential) || strings.Contains(audit.String(), token) {
		t.Fatal("audit entry exposed a credential")
	}
}
