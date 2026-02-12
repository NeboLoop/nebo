package inspector

import (
	"context"
	"encoding/json"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

// marshalPayload converts a message to JSON for display in the inspector.
// Tries protojson first for proper field naming, falls back to json.Marshal.
func marshalPayload(msg any) json.RawMessage {
	if msg == nil {
		return json.RawMessage("null")
	}
	if pm, ok := msg.(proto.Message); ok {
		b, err := protojson.Marshal(pm)
		if err == nil {
			return json.RawMessage(b)
		}
	}
	b, err := json.Marshal(msg)
	if err != nil {
		return json.RawMessage(`"<unmarshalable>"`)
	}
	return json.RawMessage(b)
}

// UnaryInterceptor returns a gRPC unary client interceptor that records calls
// to the inspector. When no subscribers exist, the fast-path skips all work.
func UnaryInterceptor(ins *Inspector, appID string) grpc.UnaryClientInterceptor {
	return func(
		ctx context.Context,
		method string,
		req, reply any,
		cc *grpc.ClientConn,
		invoker grpc.UnaryInvoker,
		opts ...grpc.CallOption,
	) error {
		if !ins.HasSubscribers() {
			return invoker(ctx, method, req, reply, cc, opts...)
		}

		start := time.Now()

		ins.Record(&Event{
			Timestamp: start,
			AppID:     appID,
			Method:    method,
			Type:      "unary",
			Direction: "request",
			Payload:   marshalPayload(req),
		})

		err := invoker(ctx, method, req, reply, cc, opts...)
		dur := time.Since(start)

		e := &Event{
			Timestamp:  time.Now(),
			AppID:      appID,
			Method:     method,
			Type:       "unary",
			Direction:  "response",
			Payload:    marshalPayload(reply),
			DurationMs: dur.Milliseconds(),
		}
		if err != nil {
			e.Error = err.Error()
		}
		ins.Record(e)
		return err
	}
}

// StreamInterceptor returns a gRPC stream client interceptor that records
// individual stream messages to the inspector.
func StreamInterceptor(ins *Inspector, appID string) grpc.StreamClientInterceptor {
	return func(
		ctx context.Context,
		desc *grpc.StreamDesc,
		cc *grpc.ClientConn,
		method string,
		streamer grpc.Streamer,
		opts ...grpc.CallOption,
	) (grpc.ClientStream, error) {
		if !ins.HasSubscribers() {
			return streamer(ctx, desc, cc, method, opts...)
		}

		ins.Record(&Event{
			Timestamp: time.Now(),
			AppID:     appID,
			Method:    method,
			Type:      "stream_open",
			Direction: "request",
			Payload:   json.RawMessage("null"),
		})

		cs, err := streamer(ctx, desc, cc, method, opts...)
		if err != nil {
			ins.Record(&Event{
				Timestamp: time.Now(),
				AppID:     appID,
				Method:    method,
				Type:      "stream_open",
				Direction: "response",
				Error:     err.Error(),
			})
			return nil, err
		}

		return &wrappedStream{
			ClientStream: cs,
			ins:          ins,
			appID:        appID,
			method:       method,
		}, nil
	}
}

// wrappedStream intercepts SendMsg/RecvMsg on a gRPC client stream.
type wrappedStream struct {
	grpc.ClientStream
	ins     *Inspector
	appID   string
	method  string
	sendSeq int
	recvSeq int
}

func (w *wrappedStream) SendMsg(m any) error {
	if w.ins.HasSubscribers() {
		w.sendSeq++
		w.ins.Record(&Event{
			Timestamp: time.Now(),
			AppID:     w.appID,
			Method:    w.method,
			Type:      "stream_send",
			Direction: "request",
			Payload:   marshalPayload(m),
			StreamSeq: w.sendSeq,
		})
	}
	return w.ClientStream.SendMsg(m)
}

func (w *wrappedStream) RecvMsg(m any) error {
	err := w.ClientStream.RecvMsg(m)
	if w.ins.HasSubscribers() {
		w.recvSeq++
		e := &Event{
			Timestamp: time.Now(),
			AppID:     w.appID,
			Method:    w.method,
			Type:      "stream_recv",
			Direction: "response",
			Payload:   marshalPayload(m),
			StreamSeq: w.recvSeq,
		}
		if err != nil {
			e.Error = err.Error()
		}
		w.ins.Record(e)
	}
	return err
}
