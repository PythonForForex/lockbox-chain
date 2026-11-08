package lockbox

import (
	"encoding/json"
	"net/http"

	"github.com/gorilla/mux"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/cosmos/cosmos-sdk/client"
)

type assetCapJsonBody struct {
	Asset        string `json:"asset"`
	MaxOrderSize string `json:"max_order_size"`
	MaxDailySize string `json:"max_daily_size"`
}

type policyJsonBody struct {
	KeyRef       string             `json:"key_ref"`
	DelegateKey  string             `json:"delegate_key"`
	Epoch        uint64             `json:"epoch"`
	Venues       []string           `json:"venues"`
	VaultAddress string             `json:"vault_address"`
	AssetCaps    []assetCapJsonBody `json:"asset_caps"`
	ExpiresAt    uint64             `json:"expires_at"`
	Revoked      bool               `json:"revoked"`
	RevokedAt    uint64             `json:"revoked_at"`
}

type keyRefJsonBody struct {
	KeyRef       string   `json:"key_ref"`
	AccountId    string   `json:"account_id"`
	VenueAddress string   `json:"venue_address"`
	Committee    []string `json:"committee"`
	Threshold    uint32   `json:"threshold"`
	PolicyEpoch  uint64   `json:"policy_epoch"`
}

type accountJsonBody struct {
	AccountId       string `json:"account_id"`
	RegistrationKey string `json:"registration_key"`
	OwnerAddress    string `json:"owner_address"`
}

func emptyListInsteadOfNull(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}

func assetCapJsonBodies(assetCaps []*AssetCap) []assetCapJsonBody {
	bodies := make([]assetCapJsonBody, 0, len(assetCaps))
	for _, assetCap := range assetCaps {
		bodies = append(bodies, assetCapJsonBody{Asset: assetCap.Asset, MaxOrderSize: assetCap.MaxOrderSize, MaxDailySize: assetCap.MaxDailySize})
	}
	return bodies
}

func writeJsonResponse(writer http.ResponseWriter, statusCode int, body any) {
	writer.Header().Set("Content-Type", "application/json")
	writer.WriteHeader(statusCode)
	_ = json.NewEncoder(writer).Encode(body)
}

func writeQueryError(writer http.ResponseWriter, err error) {
	statusCode := http.StatusInternalServerError
	if status.Code(err) == codes.NotFound {
		statusCode = http.StatusNotFound
	}
	writeJsonResponse(writer, statusCode, map[string]string{"error": err.Error()})
}

func RegisterRestRoutes(clientContext client.Context, router *mux.Router) {
	queryClient := NewQueryClient(clientContext)

	router.HandleFunc("/lockbox/v1/policy/{key_ref}/{delegate_key}", func(writer http.ResponseWriter, request *http.Request) {
		pathValues := mux.Vars(request)
		response, err := queryClient.Policy(request.Context(), &QueryPolicyRequest{KeyRef: pathValues["key_ref"], DelegateKey: pathValues["delegate_key"]})
		if err != nil {
			writeQueryError(writer, err)
			return
		}
		writeJsonResponse(writer, http.StatusOK, policyJsonBody{
			KeyRef:       response.Policy.KeyRef,
			DelegateKey:  response.Policy.DelegateKey,
			Epoch:        response.Policy.Epoch,
			Venues:       emptyListInsteadOfNull(response.Policy.Venues),
			VaultAddress: response.Policy.VaultAddress,
			AssetCaps:    assetCapJsonBodies(response.Policy.AssetCaps),
			ExpiresAt:    response.Policy.ExpiresAt,
			Revoked:      response.Policy.Revoked,
			RevokedAt:    response.Policy.RevokedAt,
		})
	}).Methods(http.MethodGet)

	router.HandleFunc("/lockbox/v1/account/{account_id}", func(writer http.ResponseWriter, request *http.Request) {
		response, err := queryClient.Account(request.Context(), &QueryAccountRequest{AccountId: mux.Vars(request)["account_id"]})
		if err != nil {
			writeQueryError(writer, err)
			return
		}
		writeJsonResponse(writer, http.StatusOK, accountJsonBody{
			AccountId:       response.Account.AccountId,
			RegistrationKey: response.Account.RegistrationKey,
			OwnerAddress:    response.Account.OwnerAddress,
		})
	}).Methods(http.MethodGet)

	router.HandleFunc("/lockbox/v1/keyref/{key_ref}", func(writer http.ResponseWriter, request *http.Request) {
		response, err := queryClient.KeyRef(request.Context(), &QueryKeyRefRequest{KeyRef: mux.Vars(request)["key_ref"]})
		if err != nil {
			writeQueryError(writer, err)
			return
		}
		writeJsonResponse(writer, http.StatusOK, keyRefJsonBody{
			KeyRef:       response.KeyRef.KeyRef,
			AccountId:    response.KeyRef.AccountId,
			VenueAddress: response.KeyRef.VenueAddress,
			Committee:    emptyListInsteadOfNull(response.KeyRef.Committee),
			Threshold:    response.KeyRef.Threshold,
			PolicyEpoch:  response.PolicyEpoch,
		})
	}).Methods(http.MethodGet)
}
