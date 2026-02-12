package broker

// OAuthProvider describes a vendor's OAuth 2.0 endpoints and credentials.
type OAuthProvider struct {
	Name                  string
	AuthorizationEndpoint string
	TokenEndpoint         string
	ClientID              string
	ClientSecret          string
	TenantID              string // Microsoft only
	SupportsPKCE          bool
}

// BuiltinProviders returns the hardcoded OAuth endpoint configs for supported vendors.
// Client credentials come from Nebo's config.yaml.
func BuiltinProviders() map[string]OAuthProvider {
	return map[string]OAuthProvider{
		"google": {
			Name:                  "Google",
			AuthorizationEndpoint: "https://accounts.google.com/o/oauth2/v2/auth",
			TokenEndpoint:         "https://oauth2.googleapis.com/token",
			SupportsPKCE:          true,
		},
		"microsoft": {
			Name:                  "Microsoft",
			AuthorizationEndpoint: "https://login.microsoftonline.com/{tenant}/oauth2/v2/authorize",
			TokenEndpoint:         "https://login.microsoftonline.com/{tenant}/oauth2/v2/token",
			SupportsPKCE:          true,
		},
		"github": {
			Name:                  "GitHub",
			AuthorizationEndpoint: "https://github.com/login/oauth/authorize",
			TokenEndpoint:         "https://github.com/login/oauth/access_token",
			SupportsPKCE:          false,
		},
	}
}
