/**
 * JavaScript Executor Worker
 *
 * Runs inside workerd and handles code execution requests.
 * Streams results as NDJSON events.
 */

/**
 * Parse V8 stack trace into structured StackFrame objects
 * @param {string} stack - The stack trace string
 * @returns {Array<{function_name: string|null, location: {line: number, column: number, filename: string|null}|null}>}
 */
function parseStack(stack) {
  if (!stack) return [];

  const frames = [];
  const lines = stack.split('\n');

  for (const line of lines) {
    // Skip the error message line
    if (!line.trim().startsWith('at ')) continue;

    const trimmed = line.trim().slice(3); // Remove 'at '

    // Try to parse different V8 stack formats:
    // 1. "functionName (filename:line:column)"
    // 2. "filename:line:column" (anonymous)
    // 3. "functionName (native)" or "native"
    // 4. "async functionName (filename:line:column)"

    let functionName = null;
    let location = null;

    // Handle async prefix
    let remaining = trimmed;
    if (remaining.startsWith('async ')) {
      remaining = remaining.slice(6);
    }

    // Check for "name (location)" format
    const parenMatch = remaining.match(/^(.+?)\s+\((.+)\)$/);
    if (parenMatch) {
      functionName = parenMatch[1] || null;
      const locStr = parenMatch[2];

      if (locStr !== 'native' && locStr !== '[native code]') {
        location = parseLocation(locStr);
      }
    } else {
      // Direct location format or native
      if (remaining !== 'native' && remaining !== '[native code]') {
        location = parseLocation(remaining);
      }
    }

    frames.push({
      function_name: functionName,
      location
    });
  }

  return frames;
}

/**
 * Parse a location string like "filename:line:column" or "<anonymous>:1:5"
 * @param {string} locStr
 * @returns {{line: number, column: number, filename: string|null}|null}
 */
function parseLocation(locStr) {
  // Match patterns like:
  // - "file.js:10:5"
  // - "<anonymous>:1:10"
  // - "eval at something (file.js:1:1), <anonymous>:1:5"

  // Handle eval locations - take the inner location
  if (locStr.includes(', ')) {
    locStr = locStr.split(', ').pop();
  }

  // Match filename:line:column
  const match = locStr.match(/^(.+):(\d+):(\d+)$/);
  if (match) {
    const filename = match[1] === '<anonymous>' ? null : match[1];
    return {
      line: parseInt(match[2], 10),
      column: parseInt(match[3], 10),
      filename
    };
  }

  // Match filename:line (no column)
  const lineMatch = locStr.match(/^(.+):(\d+)$/);
  if (lineMatch) {
    const filename = lineMatch[1] === '<anonymous>' ? null : lineMatch[1];
    return {
      line: parseInt(lineMatch[2], 10),
      column: 0,
      filename
    };
  }

  return null;
}

/**
 * Create a wrapped console that emits events to a writer
 * @param {WritableStreamDefaultWriter} writer
 * @param {TextEncoder} encoder
 * @returns {Console}
 */
function createWrappedConsole(writer, encoder) {
  const emit = async (level, args) => {
    const event = {
      type: 'console',
      level,
      args: args.map(arg => {
        try {
          // Handle various types
          if (arg === undefined) return null;
          if (arg instanceof Error) {
            return { message: arg.message, name: arg.name, stack: arg.stack };
          }
          // Try to serialize, fall back to string representation
          return JSON.parse(JSON.stringify(arg));
        } catch {
          return String(arg);
        }
      }),
      timestamp_ms: Date.now()
    };

    try {
      await writer.write(encoder.encode(JSON.stringify(event) + '\n'));
    } catch {
      // Writer may be closed, ignore
    }
  };

  return {
    log: (...args) => emit('log', args),
    info: (...args) => emit('info', args),
    warn: (...args) => emit('warn', args),
    error: (...args) => emit('error', args),
    debug: (...args) => emit('log', args), // Map debug to log
    trace: (...args) => emit('log', args), // Map trace to log
  };
}

/**
 * Execute user code and stream results
 * @param {string} code - The JavaScript code to execute
 * @param {object} context - Context data to pass to the code
 * @param {object} limits - Execution limits (not enforced in JS, handled by workerd)
 * @param {object} bindings - Additional bindings (reserved for future use)
 * @returns {ReadableStream}
 */
function executeCode(code, context, limits, bindings) {
  const { readable, writable } = new TransformStream();
  const writer = writable.getWriter();
  const encoder = new TextEncoder();

  const wrappedConsole = createWrappedConsole(writer, encoder);

  (async () => {
    try {
      // Create an async function from the user code
      // This allows users to use await at the top level
      const AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;

      // The function receives console and ctx as arguments
      // We wrap the code to capture the last expression value
      const wrappedCode = `
        "use strict";
        ${code}
      `;

      const fn = new AsyncFunction('console', 'ctx', wrappedCode);

      // Execute with the wrapped console and context data
      const result = await fn(wrappedConsole, context?.data ?? {});

      // Emit the returned value
      const returnedEvent = {
        type: 'returned',
        value: result === undefined ? null : result
      };

      try {
        // Try to serialize the result
        await writer.write(encoder.encode(JSON.stringify(returnedEvent) + '\n'));
      } catch (serializeError) {
        // If serialization fails, convert to string
        const fallbackEvent = {
          type: 'returned',
          value: String(result)
        };
        await writer.write(encoder.encode(JSON.stringify(fallbackEvent) + '\n'));
      }

    } catch (error) {
      // Parse the error into a structured format
      const errorEvent = {
        type: 'error',
        message: error.message || String(error),
        name: error.name || 'Error',
        location: null,
        stack: []
      };

      if (error.stack) {
        const frames = parseStack(error.stack);
        errorEvent.stack = frames;

        // Try to extract location from the first frame
        if (frames.length > 0 && frames[0].location) {
          errorEvent.location = frames[0].location;
        }
      }

      await writer.write(encoder.encode(JSON.stringify(errorEvent) + '\n'));
    } finally {
      await writer.close();
    }
  })();

  return readable;
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    // Health check endpoint
    if (url.pathname === '/health') {
      return new Response(
        JSON.stringify({ status: 'healthy' }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' }
        }
      );
    }

    // Execute endpoint
    if (url.pathname === '/execute') {
      if (request.method !== 'POST') {
        return new Response(
          JSON.stringify({ error: 'Method not allowed' }),
          {
            status: 405,
            headers: { 'Content-Type': 'application/json' }
          }
        );
      }

      let body;
      try {
        body = await request.json();
      } catch (parseError) {
        return new Response(
          JSON.stringify({ error: 'Invalid JSON body' }),
          {
            status: 400,
            headers: { 'Content-Type': 'application/json' }
          }
        );
      }

      const { code, limits, context, bindings } = body;

      if (typeof code !== 'string') {
        return new Response(
          JSON.stringify({ error: 'Missing or invalid "code" field' }),
          {
            status: 400,
            headers: { 'Content-Type': 'application/json' }
          }
        );
      }

      // Execute the code and return streaming response
      const stream = executeCode(code, context, limits, bindings);

      return new Response(stream, {
        status: 200,
        headers: {
          'Content-Type': 'application/x-ndjson',
          'Transfer-Encoding': 'chunked'
        }
      });
    }

    // Unknown endpoint
    return new Response(
      JSON.stringify({ error: 'Not found' }),
      {
        status: 404,
        headers: { 'Content-Type': 'application/json' }
      }
    );
  }
};
