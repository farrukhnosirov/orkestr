/**
 * Platform seams of the mobile core (ADR-0032). The RN app plugs in Expo
 * implementations (src/adapters); tests and the Node acceptance script use the
 * in-memory ones below. Nothing here imports react-native.
 */

/** Small secret storage (expo-secure-store). Keys: `[A-Za-z0-9._-]+`. */
export interface SecureKV {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
}

/** The user refused / failed the biometric prompt. */
export class BiometricDeniedError extends Error {
  readonly code = "biometric_denied";
  constructor(message = "biometric authentication denied") {
    super(message);
    this.name = "BiometricDeniedError";
  }
}

/**
 * Biometric-gated secret storage: on device a SecureStore item with
 * `requireAuthentication: true` (Keystore/Keychain enforce the biometric),
 * enrollment check via expo-local-authentication.
 */
export interface BiometricGate {
  /** Hardware present and a biometric is enrolled. */
  available(): Promise<boolean>;
  /** Writes a gated secret (may prompt on iOS). */
  setSecret(key: string, value: string): Promise<void>;
  /** Reads a gated secret behind the biometric prompt; rejects with `BiometricDeniedError`. */
  getSecret(key: string, prompt: string): Promise<string | null>;
  deleteSecret(key: string): Promise<void>;
}

export interface LocalNotification {
  id: string;
  title: string;
  body: string;
  /** Routed on tap (see `routeForNotification`). */
  data: Record<string, string>;
}

export type NativePushToken = { transport: "fcm" | "apns"; token: string };

/** Local notifications + native push token (expo-notifications). */
export interface Notifier {
  requestPermission(): Promise<boolean>;
  show(n: LocalNotification): Promise<void>;
  /** Native FCM/APNs token, or `null` when this build has no push credentials. */
  nativeToken(): Promise<NativePushToken | null>;
}

export interface Recording {
  base64: string;
  mimeType: string;
}

/** Microphone recorder (expo-audio). */
export interface Recorder {
  start(): Promise<void>;
  stop(): Promise<Recording>;
}

/** Text-to-speech (expo-speech). */
export interface Speaker {
  speak(text: string, language: string): void;
}

// ---------------------------------------------------------------- in-memory

export class MemorySecureKV implements SecureKV {
  readonly data = new Map<string, string>();
  async get(key: string) {
    return this.data.get(key) ?? null;
  }
  async set(key: string, value: string) {
    this.data.set(key, value);
  }
  async delete(key: string) {
    this.data.delete(key);
  }
}

/** Test gate: `allow` decides each prompt; `prompts` counts biometric prompts. */
export class MemoryBiometricGate implements BiometricGate {
  readonly data = new Map<string, string>();
  allow = true;
  enrolled = true;
  prompts = 0;
  async available() {
    return this.enrolled;
  }
  async setSecret(key: string, value: string) {
    this.data.set(key, value);
  }
  async getSecret(key: string, _prompt: string) {
    this.prompts += 1;
    if (!this.allow) throw new BiometricDeniedError();
    return this.data.get(key) ?? null;
  }
  async deleteSecret(key: string) {
    this.data.delete(key);
  }
}

export class MemoryNotifier implements Notifier {
  readonly shown: LocalNotification[] = [];
  granted = true;
  token: NativePushToken | null = null;
  async requestPermission() {
    return this.granted;
  }
  async show(n: LocalNotification) {
    this.shown.push(n);
  }
  async nativeToken() {
    return this.token;
  }
}

export class MemoryRecorder implements Recorder {
  recording = false;
  constructor(public next: Recording = { base64: "", mimeType: "audio/wav" }) {}
  async start() {
    this.recording = true;
  }
  async stop() {
    this.recording = false;
    return this.next;
  }
}

export class MemorySpeaker implements Speaker {
  readonly spoken: { text: string; language: string }[] = [];
  speak(text: string, language: string) {
    this.spoken.push({ text, language });
  }
}
