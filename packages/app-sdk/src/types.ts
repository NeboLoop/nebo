export interface AgentResponse {
  text: string;
  tools?: Array<{ name: string; result: unknown }>;
}

export interface InvokeOptions {
  agent?: string;
  data?: Record<string, unknown>;
}

export interface JanusMessage {
  role: 'user' | 'assistant' | 'system';
  content: string;
}

export interface JanusOptions {
  messages: JanusMessage[];
  model?: string;
  max_tokens?: number;
}

export interface StreamChunk {
  text: string;
  done: boolean;
}
