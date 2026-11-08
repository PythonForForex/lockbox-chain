package lockbox

import (
	"context"
	"errors"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"cosmossdk.io/collections"
)

const (
	defaultLogEntriesPageSize = 100
	maximumLogEntriesPageSize = 1000
)

type queryServer struct {
	keeper *Keeper
}

func NewQueryServer(keeper *Keeper) QueryServer {
	return &queryServer{keeper: keeper}
}

func notFoundOrInternal(err error, what string) error {
	if errors.Is(err, collections.ErrNotFound) {
		return status.Errorf(codes.NotFound, "%s not found", what)
	}
	return status.Error(codes.Internal, err.Error())
}

func (server *queryServer) Account(ctx context.Context, request *QueryAccountRequest) (*QueryAccountResponse, error) {
	account, err := server.keeper.Accounts.Get(ctx, request.AccountId)
	if err != nil {
		return nil, notFoundOrInternal(err, "account")
	}
	return &QueryAccountResponse{Account: &account}, nil
}

func (server *queryServer) Signer(ctx context.Context, request *QuerySignerRequest) (*QuerySignerResponse, error) {
	signer, err := server.keeper.Signers.Get(ctx, request.SignerId)
	if err != nil {
		return nil, notFoundOrInternal(err, "signer")
	}
	return &QuerySignerResponse{Signer: &signer}, nil
}

func (server *queryServer) KeyRef(ctx context.Context, request *QueryKeyRefRequest) (*QueryKeyRefResponse, error) {
	keyRef, err := server.keeper.KeyRefs.Get(ctx, request.KeyRef)
	if err != nil {
		return nil, notFoundOrInternal(err, "key_ref")
	}
	policyEpoch, err := server.keeper.currentPolicyEpoch(ctx, request.KeyRef)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &QueryKeyRefResponse{KeyRef: &keyRef, PolicyEpoch: policyEpoch}, nil
}

func (server *queryServer) Policy(ctx context.Context, request *QueryPolicyRequest) (*QueryPolicyResponse, error) {
	policy, err := server.keeper.Policies.Get(ctx, collections.Join(request.KeyRef, request.DelegateKey))
	if err != nil {
		return nil, notFoundOrInternal(err, "policy")
	}
	return &QueryPolicyResponse{Policy: &policy}, nil
}

func (server *queryServer) Release(ctx context.Context, request *QueryReleaseRequest) (*QueryReleaseResponse, error) {
	release, err := server.keeper.AllowedReleases.Get(ctx, request.BinaryHash)
	if err != nil {
		return nil, notFoundOrInternal(err, "release")
	}
	return &QueryReleaseResponse{Release: &release}, nil
}

func (server *queryServer) LogEntries(ctx context.Context, request *QueryLogEntriesRequest) (*QueryLogEntriesResponse, error) {
	pageSize := request.Limit
	if pageSize == 0 {
		pageSize = defaultLogEntriesPageSize
	}
	if pageSize > maximumLogEntriesPageSize {
		pageSize = maximumLogEntriesPageSize
	}
	iterator, err := server.keeper.LogEntries.Iterate(ctx, new(collections.Range[uint64]).StartInclusive(request.FirstSequence))
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	defer iterator.Close()
	response := &QueryLogEntriesResponse{NextSequence: request.FirstSequence}
	for ; iterator.Valid() && uint32(len(response.Entries)) < pageSize; iterator.Next() {
		keyValue, err := iterator.KeyValue()
		if err != nil {
			return nil, status.Error(codes.Internal, err.Error())
		}
		entry := keyValue.Value
		response.Entries = append(response.Entries, &entry)
		response.NextSequence = keyValue.Key + 1
	}
	return response, nil
}

func (server *queryServer) Anchor(ctx context.Context, request *QueryAnchorRequest) (*QueryAnchorResponse, error) {
	anchor, err := server.keeper.TapeAnchors.Get(ctx, request.SegmentId)
	if err != nil {
		return nil, notFoundOrInternal(err, "segment")
	}
	return &QueryAnchorResponse{Anchor: &anchor}, nil
}
