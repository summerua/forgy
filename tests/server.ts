// server.ts - Test server for forgy load testing
// Run with: bun run server.ts

import { serve } from "bun";
import { randomBytes } from "crypto";

// Configuration
const PORT = process.env.PORT ? parseInt(process.env.PORT) : 3000;
const VERBOSE = process.env.VERBOSE === "true";
const DELAY_MS = process.env.DELAY_MS ? parseInt(process.env.DELAY_MS) : 0;
const ERROR_RATE = process.env.ERROR_RATE ? parseFloat(process.env.ERROR_RATE) : 0;

// Statistics
let requestCount = 0;
let startTime = Date.now();
const statusCodes: Record<number, number> = {};
const methodCounts: Record<string, number> = {};
const pathCounts: Record<string, number> = {};
const responseTimes: number[] = [];

// Colors for terminal output
const colors = {
  reset: "\x1b[0m",
  bright: "\x1b[1m",
  dim: "\x1b[2m",
  red: "\x1b[31m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  blue: "\x1b[34m",
  magenta: "\x1b[35m",
  cyan: "\x1b[36m",
  white: "\x1b[37m",
};

// Helper function to format bytes
function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`;
}

// Helper function to get color based on status code
function getStatusColor(status: number): string {
  if (status >= 500) return colors.red;
  if (status >= 400) return colors.yellow;
  if (status >= 300) return colors.blue;
  if (status >= 200) return colors.green;
  return colors.white;
}

// Helper function to get method color
function getMethodColor(method: string): string {
  switch (method) {
    case "GET": return colors.green;
    case "POST": return colors.blue;
    case "PUT": return colors.yellow;
    case "DELETE": return colors.red;
    case "PATCH": return colors.magenta;
    default: return colors.white;
  }
}

// Simulate processing delay
async function simulateDelay(ms: number): Promise<void> {
  if (ms > 0) {
    await Bun.sleep(ms);
  }
}

// Simulate random errors based on ERROR_RATE
function shouldReturnError(): boolean {
  return ERROR_RATE > 0 && Math.random() < ERROR_RATE;
}

// Print server statistics
function printStats() {
  const uptime = (Date.now() - startTime) / 1000;
  const rps = requestCount / uptime;
  const avgResponseTime = responseTimes.length > 0 
    ? responseTimes.reduce((a, b) => a + b, 0) / responseTimes.length 
    : 0;

  console.log(`\n${colors.bright}📊 Server Statistics${colors.reset}`);
  console.log(`${colors.dim}${"─".repeat(50)}${colors.reset}`);
  console.log(`Uptime: ${colors.cyan}${uptime.toFixed(2)}s${colors.reset}`);
  console.log(`Total Requests: ${colors.cyan}${requestCount}${colors.reset}`);
  console.log(`Requests/sec: ${colors.cyan}${rps.toFixed(2)}${colors.reset}`);
  console.log(`Avg Response Time: ${colors.cyan}${avgResponseTime.toFixed(2)}ms${colors.reset}`);
  
  if (Object.keys(statusCodes).length > 0) {
    console.log(`\n${colors.bright}Status Codes:${colors.reset}`);
    Object.entries(statusCodes)
      .sort(([a], [b]) => parseInt(a) - parseInt(b))
      .forEach(([code, count]) => {
        const percentage = ((count / requestCount) * 100).toFixed(2);
        const color = getStatusColor(parseInt(code));
        console.log(`  ${color}${code}${colors.reset}: ${count} (${percentage}%)`);
      });
  }

  if (Object.keys(methodCounts).length > 0) {
    console.log(`\n${colors.bright}Methods:${colors.reset}`);
    Object.entries(methodCounts)
      .sort(([a], [b]) => b.localeCompare(a))
      .forEach(([method, count]) => {
        const percentage = ((count / requestCount) * 100).toFixed(2);
        const color = getMethodColor(method);
        console.log(`  ${color}${method}${colors.reset}: ${count} (${percentage}%)`);
      });
  }

  if (Object.keys(pathCounts).length > 0) {
    console.log(`\n${colors.bright}Top Paths:${colors.reset}`);
    Object.entries(pathCounts)
      .sort(([, a], [, b]) => b - a)
      .slice(0, 5)
      .forEach(([path, count]) => {
        const percentage = ((count / requestCount) * 100).toFixed(2);
        console.log(`  ${colors.dim}${path}${colors.reset}: ${count} (${percentage}%)`);
      });
  }
  
  console.log(`${colors.dim}${"─".repeat(50)}${colors.reset}\n`);
}

// Main server
const server = serve({
  port: PORT,
  
  async fetch(request: Request): Promise<Response> {
    const startTime = performance.now();
    requestCount++;
    
    // Parse request details
    const url = new URL(request.url);
    const method = request.method;
    const path = url.pathname;
    const requestId = randomBytes(8).toString("hex");
    
    // Update statistics
    methodCounts[method] = (methodCounts[method] || 0) + 1;
    pathCounts[path] = (pathCounts[path] || 0) + 1;
    
    // Get request headers
    const headers: Record<string, string> = {};
    request.headers.forEach((value, key) => {
      headers[key] = value;
    });
    
    // Get body if present
    let body: any = null;
    let bodySize = 0;
    if (["POST", "PUT", "PATCH"].includes(method)) {
      const contentType = headers["content-type"] || "";
      try {
        if (contentType.includes("application/json")) {
          const text = await request.text();
          bodySize = text.length;
          body = JSON.parse(text);
        } else if (contentType.includes("text/")) {
          body = await request.text();
          bodySize = body.length;
        } else {
          const blob = await request.blob();
          bodySize = blob.size;
          body = `<binary data: ${formatBytes(bodySize)}>`;
        }
      } catch (error) {
        body = "<failed to parse body>";
      }
    }
    
    // Simulate delay if configured
    if (DELAY_MS > 0) {
      await simulateDelay(DELAY_MS);
    }
    
    // Determine response status
    let status = 200;
    let errorMessage = null;
    
    if (shouldReturnError()) {
      // Randomly select an error status
      const errorStatuses = [400, 401, 403, 404, 429, 500, 502, 503];
      status = errorStatuses[Math.floor(Math.random() * errorStatuses.length)];
      errorMessage = `Simulated error (${ERROR_RATE * 100}% error rate)`;
    }
    
    // Special endpoints
    if (path === "/health") {
      status = 200;
    } else if (path === "/slow") {
      await simulateDelay(2000);
    } else if (path.startsWith("/status/")) {
      const requestedStatus = parseInt(path.split("/")[2]);
      if (!isNaN(requestedStatus) && requestedStatus >= 100 && requestedStatus < 600) {
        status = requestedStatus;
      }
    }
    
    // Calculate response time
    const responseTime = performance.now() - startTime;
    responseTimes.push(responseTime);
    if (responseTimes.length > 1000) {
      responseTimes.shift(); // Keep only last 1000 response times
    }
    
    // Update status code statistics
    statusCodes[status] = (statusCodes[status] || 0) + 1;
    
    // Prepare response
    const responseData = {
      requestId,
      timestamp: new Date().toISOString(),
      method,
      path,
      query: Object.fromEntries(url.searchParams),
      headers: VERBOSE ? headers : undefined,
      body: body,
      bodySize: bodySize > 0 ? formatBytes(bodySize) : undefined,
      processingTime: `${responseTime.toFixed(2)}ms`,
      status,
      error: errorMessage,
      server: {
        uptime: `${((Date.now() - startTime) / 1000).toFixed(2)}s`,
        totalRequests: requestCount,
        requestsPerSecond: (requestCount / ((Date.now() - startTime) / 1000)).toFixed(2),
      },
    };
    
    // Log request (configurable verbosity)
    const methodColor = getMethodColor(method);
    const statusColor = getStatusColor(status);
    const timeColor = responseTime > 1000 ? colors.red : responseTime > 500 ? colors.yellow : colors.green;
    
    console.log(
      `${colors.dim}[${new Date().toISOString()}]${colors.reset} ` +
      `${colors.bright}#${requestCount}${colors.reset} ` +
      `${methodColor}${method.padEnd(7)}${colors.reset} ` +
      `${statusColor}${status}${colors.reset} ` +
      `${colors.cyan}${path}${colors.reset} ` +
      `${timeColor}${responseTime.toFixed(2)}ms${colors.reset} ` +
      (bodySize > 0 ? `${colors.dim}[${formatBytes(bodySize)}]${colors.reset} ` : "") +
      (errorMessage ? `${colors.red}✗${colors.reset}` : `${colors.green}✓${colors.reset}`)
    );
    
    // Return response
    return new Response(
      JSON.stringify(responseData, null, 2),
      {
        status,
        headers: {
          "Content-Type": "application/json",
          "X-Request-Id": requestId,
          "X-Response-Time": responseTime.toFixed(2),
          "X-Server-Time": new Date().toISOString(),
          "Access-Control-Allow-Origin": "*",
          "Access-Control-Allow-Methods": "GET, POST, PUT, DELETE, PATCH, OPTIONS",
          "Access-Control-Allow-Headers": "*",
        },
      }
    );
  },
});

// Startup message
console.log(`${colors.bright}${colors.green}🚀 forgy Test Server${colors.reset}`);
console.log(`${colors.dim}${"─".repeat(50)}${colors.reset}`);
console.log(`Server: ${colors.cyan}http://localhost:${PORT}${colors.reset}`);
console.log(`Verbose: ${colors.cyan}${VERBOSE}${colors.reset}`);
console.log(`Delay: ${colors.cyan}${DELAY_MS}ms${colors.reset}`);
console.log(`Error Rate: ${colors.cyan}${ERROR_RATE * 100}%${colors.reset}`);
console.log(`${colors.dim}${"─".repeat(50)}${colors.reset}`);
console.log(`\n${colors.yellow}Special endpoints:${colors.reset}`);
console.log(`  /health - Always returns 200`);
console.log(`  /slow - Adds 2s delay`);
console.log(`  /status/{code} - Returns specified status code`);
console.log(`\n${colors.yellow}Environment variables:${colors.reset}`);
console.log(`  PORT=${PORT}`);
console.log(`  VERBOSE=true/false (show headers)`);
console.log(`  DELAY_MS=0 (add delay to all requests)`);
console.log(`  ERROR_RATE=0.1 (10% error rate)`);
console.log(`${colors.dim}${"─".repeat(50)}${colors.reset}\n`);

// Handle shutdown gracefully
process.on("SIGINT", () => {
  console.log(`\n${colors.yellow}Shutting down...${colors.reset}`);
  printStats();
  process.exit(0);
});

process.on("SIGTERM", () => {
  console.log(`\n${colors.yellow}Shutting down...${colors.reset}`);
  printStats();
  process.exit(0);
});