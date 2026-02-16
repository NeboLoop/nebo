package local

import (
	"bytes"
	"context"
	"crypto/tls"
	"fmt"
	"net/smtp"

	"github.com/neboloop/nebo/internal/config"

	"github.com/neboloop/nebo/internal/logging"
)

type EmailService struct {
	config config.Config
}

func NewEmailService(cfg config.Config) *EmailService {
	return &EmailService{
		config: cfg,
	}
}

type SendEmailRequest struct {
	To       string
	Subject  string
	Body     string
	TextBody string
}

type SendEmailResponse struct {
	Success   bool
	MessageID string
	Status    string
	Message   string
}

func (s *EmailService) IsConfigured() bool {
	return s.config.Email.SMTPHost != "" && s.config.Email.FromAddress != ""
}

func (s *EmailService) SendEmail(ctx context.Context, req SendEmailRequest) (*SendEmailResponse, error) {
	if !s.IsConfigured() {
		logging.Info("Email service not configured - skipping email send")
		return &SendEmailResponse{
			Success: true,
			Status:  "skipped",
			Message: "Email service not configured",
		}, nil
	}

	return s.sendViaSMTP(ctx, req)
}

func (s *EmailService) SendSimpleEmail(ctx context.Context, to, subject, htmlBody, textBody string) (*SendEmailResponse, error) {
	return s.SendEmail(ctx, SendEmailRequest{
		To:       to,
		Subject:  subject,
		Body:     htmlBody,
		TextBody: textBody,
	})
}

func (s *EmailService) sendViaSMTP(_ context.Context, req SendEmailRequest) (*SendEmailResponse, error) {
	cfg := s.config.Email

	var msg bytes.Buffer

	msg.WriteString(fmt.Sprintf("From: %s <%s>\r\n", cfg.FromName, cfg.FromAddress))
	msg.WriteString(fmt.Sprintf("To: %s\r\n", req.To))
	msg.WriteString(fmt.Sprintf("Subject: %s\r\n", req.Subject))

	if cfg.ReplyTo != "" {
		msg.WriteString(fmt.Sprintf("Reply-To: %s\r\n", cfg.ReplyTo))
	}

	msg.WriteString("MIME-Version: 1.0\r\n")

	body := req.Body
	contentType := "text/html"
	if body == "" {
		body = req.TextBody
		contentType = "text/plain"
	}

	msg.WriteString(fmt.Sprintf("Content-Type: %s; charset=UTF-8\r\n", contentType))
	msg.WriteString("\r\n")
	msg.WriteString(body)

	addr := fmt.Sprintf("%s:%d", cfg.SMTPHost, cfg.SMTPPort)

	var auth smtp.Auth
	if cfg.SMTPUser != "" && cfg.SMTPPass != "" {
		auth = smtp.PlainAuth("", cfg.SMTPUser, cfg.SMTPPass, cfg.SMTPHost)
	}

	var err error
	if cfg.SMTPPort == 465 {
		err = s.sendMailTLS(addr, auth, cfg.FromAddress, []string{req.To}, msg.Bytes())
	} else {
		err = smtp.SendMail(addr, auth, cfg.FromAddress, []string{req.To}, msg.Bytes())
	}

	if err != nil {
		logging.Errorf("SMTP send failed: %v", err)
		return nil, fmt.Errorf("failed to send email: %w", err)
	}

	logging.Infof("Email sent via SMTP to %s", req.To)
	return &SendEmailResponse{
		Success: true,
		Status:  "sent",
		Message: "Email sent via SMTP",
	}, nil
}

func (s *EmailService) sendMailTLS(addr string, auth smtp.Auth, from string, to []string, msg []byte) error {
	cfg := s.config.Email

	tlsConfig := &tls.Config{
		ServerName: cfg.SMTPHost,
	}

	conn, err := tls.Dial("tcp", addr, tlsConfig)
	if err != nil {
		return err
	}
	defer conn.Close()

	client, err := smtp.NewClient(conn, cfg.SMTPHost)
	if err != nil {
		return err
	}
	defer client.Close()

	if auth != nil {
		if err = client.Auth(auth); err != nil {
			return err
		}
	}

	if err = client.Mail(from); err != nil {
		return err
	}

	for _, addr := range to {
		if err = client.Rcpt(addr); err != nil {
			return err
		}
	}

	w, err := client.Data()
	if err != nil {
		return err
	}

	_, err = w.Write(msg)
	if err != nil {
		return err
	}

	err = w.Close()
	if err != nil {
		return err
	}

	return client.Quit()
}
