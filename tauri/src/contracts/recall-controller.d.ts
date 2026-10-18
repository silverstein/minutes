import type { RecallChatChunk } from './recall-chat';

export interface RecallChatController {
  register(): Promise<void>;
  reset(): Promise<void>;
  send(): Promise<void>;
  cancel(): Promise<void>;
  dispose(): Promise<void>;
  showWelcome(): void;
}
export interface RecallChatOptions {
  messages: HTMLElement;
  input: HTMLTextAreaElement;
  sendButton: HTMLButtonElement;
  jumpButton: HTMLButtonElement;
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
  listen(name: string, handler: (event: { payload?: unknown }) => void): Promise<() => void>;
  renderMarkdown(element: HTMLElement, text: string): void;
  report?(...messages: unknown[]): void;
}
declare global {
  interface Window {
    MinutesRecallContract: {
      readonly types: readonly string[];
      validate(value: unknown): 'valid' | 'invalid' | 'ignored' | 'unknown';
    };
    MinutesRecallChat: {
      create(options: RecallChatOptions): RecallChatController;
      decodeChunk(chunk: RecallChatChunk): object;
    };
  }
}
