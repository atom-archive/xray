declare namespace Xray {
  class TextBuffer {
    constructor(replicaId: number);
    length: number;
    getText(): string;
    splice(start: number, count: number, newText: string);
  }

  class TextEditor {
    constructor(buffer: TextBuffer, onChange: () => void);
    destroy(): void;
  }
}

declare module 'xray' {
  export = Xray;
}

interface NodeRequireFunction {
  (moduleName: 'xray'): typeof Xray;
}
