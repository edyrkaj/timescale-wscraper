# syntax=docker/dockerfile:1
FROM mcr.microsoft.com/playwright:v1.50.1-noble
WORKDIR /app
COPY package.json ./
RUN npm install --omit=dev
COPY server.js ./
ENV PORT=3001
ENV HEADLESS=0
ENV PLAYWRIGHT_BROWSER=chromium
EXPOSE 3001
CMD ["xvfb-run", "-a", "node", "server.js"]
