import * as SimplexNoise from "simplex-noise";

declare global {
    interface HTMLCanvasElement {
        captureStream(frames: number) : MediaStream;
    }
}

type RenderProperties = {
    /** Time since epoch in milliseconds */
    timestamp: number,

    width: number,
    height: number
}

abstract class VirtualCameraRenderer {
    public abstract render(canvas: CanvasRenderingContext2D, properties: RenderProperties);

    public cameraStarted() {}
    public cameraStopped() {}
}

class FPSMeter {
    private frameCount;
    private lastFrameTimestamp;

    private avgFps;

    constructor() {
        this.frameCount = 0;
        this.lastFrameTimestamp = 0;
        this.avgFps = -1;
    }

    public getFPS() : number {
        return this.avgFps;
    }

    public getFrameCount() : number {
        return this.frameCount;
    }

    public frameBegin() {
        if(this.lastFrameTimestamp !== 0) {
            let fps = 1000 / (Date.now() - this.lastFrameTimestamp);
            if(this.avgFps === -1) {
                this.avgFps = fps;
            } else {
                this.avgFps = this.avgFps * .8 + fps * .2;
            }
        }
        this.lastFrameTimestamp = Date.now();
    }

    public frameEnd() {
        this.frameCount++;
    }
}

export class VirtualCamera {
    private readonly frameRate: number;
    private readonly canvas: HTMLCanvasElement;
    private readonly stream: MediaStream;

    private readonly frameMeter: FPSMeter;

    private baseTime: number;
    private canvasContext: CanvasRenderingContext2D;
    private running: boolean;
    private renderer: VirtualCameraRenderer;
    private renderInterval;

    public constructor(frameRate: number, bounds: { width: number, height: number }) {
        this.running = false;
        this.frameRate = frameRate;
        this.canvas = document.createElement("canvas");
        this.canvas.width = bounds.width;
        this.canvas.height = bounds.height;

        this.frameMeter = new FPSMeter();

        this.createCanvas();
        this.stream = this.canvas.captureStream(this.frameRate > 0 ? this.frameRate : 60);

        //this.renderer = new RendererSnow();
        //this.renderer = new RendererSnowSimplex();
        this.renderer = new RendererClock();
    }

    public start() {
        if(this.running) {
            return;
        }

        this.baseTime = Date.now();
        this.running = true;
        this.renderer.cameraStarted();
        this.renderInterval = setInterval(() => this.doRender(), this.frameRate >= 0 ? 1000 / this.frameRate : 50);
    }

    public stop() {
        if(!this.running) {
            return;
        }
        this.running = false;
        this.renderer.cameraStopped();
        clearInterval(this.renderInterval);
        this.renderInterval = undefined;
    }

    public getMediaStream() : MediaStream {
        return this.stream;
    }

    private createCanvas() {
        /* recreate, the old one may be crashed if we would use WebGL */
        this.canvasContext = this.canvas.getContext("2d");
        this.canvasContext.imageSmoothingEnabled = false;
    }

    private doRender() {
        if(!this.canvasContext) {
            this.createCanvas();
        }
        const ctx = this.canvasContext;

        this.frameMeter.frameBegin();
        let renderProperties: RenderProperties = {
            height: this.canvas.height,
            width: this.canvas.width,
            timestamp: Date.now()
        };

        ctx.clearRect(0, 0, renderProperties.width, renderProperties.height);
        this.renderer.render(ctx, renderProperties);
        false && this.renderDebugStats(ctx, renderProperties);

        this.frameMeter.frameEnd();
    }

    private renderDebugStats(ctx: CanvasRenderingContext2D, properties: RenderProperties) {
        let fontSize = 35;
        ctx.font = `${fontSize}px Consolas,monaco,monospace`;
        ctx.fillStyle = "magenta";
        ctx.strokeStyle = "black";
        ctx.lineWidth = 1;
        ctx.textAlign = "left";
        ctx.textBaseline = "top";

        let xOffset = 20;
        let yOffset = 20;
        {
            let text = "FPS: " + this.frameMeter.getFPS().toFixed(0);
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);

            xOffset += 5 * fontSize;
        }

        {
            if(xOffset + 6 * fontSize > properties.width) {
                xOffset = 20;
                yOffset += fontSize;
            }

            let text = "Time: ";
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            xOffset += 6 * fontSize;

            text = (Date.now() - this.baseTime).toString();
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            ctx.textAlign = "left";
            xOffset += fontSize;
        }

        {
            if(xOffset + 8 * fontSize > properties.width) {
                xOffset = 20;
                yOffset += fontSize;
            }

            let text = "Frame: ";
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            xOffset += 8 * fontSize;

            text = this.frameMeter.getFrameCount().toString();
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            ctx.textAlign = "left";
            xOffset += fontSize;
        }
    }
}

function rgba(red: number, green: number, blue: number, alpha: number) {
    return ((alpha << 24) | (blue << 16) | (green << 8) | (red << 0)) >>> 0;
}
const SnowColors = [
    rgba(255, 255, 255, 255),
    rgba(220, 220, 220, 255),
    rgba(170, 170, 170, 255),
    rgba(120, 120, 120, 255),
    rgba(0, 0, 0, 255)
];

class RendererSnow extends VirtualCameraRenderer {
    render(canvas: CanvasRenderingContext2D, properties: RenderProperties) {
        const data = canvas.getImageData(0, 0, properties.width, properties.height);
        if(data.data.byteOffset % 4 !== 0) {
            throw "image data byte offset is invalid";
        }

        const buffer = new Uint32Array(data.data.buffer, data.data.byteOffset / 4, data.data.byteLength / 4);
        for(let index = 0; index < buffer.length; index++) {
            buffer[index] = SnowColors[(Math.random() * SnowColors.length) | 0];
        }

        canvas.putImageData(data, 0, 0);
    }
}

type RendererSnowSimplexConfiguration = {
    gridScale: number,
    timeScale: number
}

class RendererSnowSimplex extends VirtualCameraRenderer {
    public static readonly ConfigSlow: RendererSnowSimplexConfiguration = { gridScale: 0.005, timeScale: 0.0001 };
    public static readonly ConfigSuperFast: RendererSnowSimplexConfiguration = { gridScale: .1, timeScale: 0.1 };
    public static readonly ConfigSuperFastRandom: RendererSnowSimplexConfiguration = { gridScale: 10, timeScale: 1 };

    private readonly config: RendererSnowSimplexConfiguration;
    private readonly noise: SimplexNoise;
    private readonly baseTimestamp: number;

    constructor() {
        super();
        this.config = RendererSnowSimplex.ConfigSlow;
        this.noise = new SimplexNoise();
        this.baseTimestamp = Date.now();
    }

    render(canvas: CanvasRenderingContext2D, properties: RenderProperties) {
        const data = canvas.getImageData(0, 0, properties.width, properties.height);
        if(data.data.byteOffset % 4 !== 0) {
            throw "image data byte offset is invalid";
        }

        const buffer = new Uint32Array(data.data.buffer, data.data.byteOffset / 4, data.data.byteLength / 4);
        for(let y = 0; y < data.height; y++) {
            for(let x = 0; x < data.width; x++) {
                buffer[y * data.width + x] = SnowColors[Math.abs(this.noise.noise3D(x * this.config.gridScale, y * this.config.gridScale, (this.baseTimestamp - properties.timestamp) * this.config.timeScale) * SnowColors.length) | 0];
            }
        }

        canvas.putImageData(data, 0, 0);
    }
}

/* Well this renderer has been inspired by the song Time Is Ticking (Secret Layor Remix) (https://www.youtube.com/watch?v=QmWzC9XYiIQ) */
class RendererClock extends VirtualCameraRenderer {
    private readonly baseTime = Date.now();
    private smoothSecondPointer = true;
    private fuckingFastClockBecauseIveNoTimeToWait = false;

    render(canvas: CanvasRenderingContext2D, properties: RenderProperties) {
        canvas.fillStyle = "white";
        canvas.fillRect(0, 0, properties.width, properties.height);

        /* The outer circle */
        {
            canvas.beginPath();
            for(let sub of [10, 11, 12]) {
                canvas.ellipse(properties.width / 2, properties.height / 2, properties.width / 2 - sub, properties.height / 2 - sub, 0, 0, 360);
            }
            canvas.stroke();
            canvas.closePath();
        }

        /* The numbers */
        {
            canvas.textAlign = "center";
            canvas.textBaseline = "middle";
            canvas.font = `20px Consolas,monaco,monospace`;
            canvas.strokeStyle = "black";
            canvas.fillStyle = "black";

            let numberDistance = 45;
            for(let number = 1; number <= 12; number++) {
                const index = number / 6 * Math.PI;
                const x = Math.sin(index) * (properties.width / 2 - numberDistance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - numberDistance) + properties.height / 2;

                canvas.fillText(number.toString(), x, y);
            }

            let lineDistance = 20;
            canvas.lineWidth = 2;
            canvas.beginPath();
            for(let minute = 0; minute < 60; minute++) {
                const index = minute / 30 * Math.PI;
                let distance = lineDistance;
                if(minute % 5 === 0) {
                    distance *= 1.5;
                }

                const sx = Math.sin(index) * (properties.width / 2 - 12) + properties.width / 2;
                const sy = -Math.cos(index) * (properties.height / 2 - 12) + properties.height / 2;
                const dx = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const dy = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;

                canvas.moveTo(sx, sy);
                canvas.lineTo(dx, dy);
            }
            canvas.stroke();
            canvas.closePath();
        }

        /* The pointers */
        {
            let currentTimestamp: Date;
            if(this.fuckingFastClockBecauseIveNoTimeToWait) {
                currentTimestamp = new Date(Date.now() + (Date.now() - this.baseTime) * 100);
            } else {
                currentTimestamp = new Date();
            }
            canvas.beginPath();

            /* the hour pointer */
            {
                let distance = 120;
                canvas.lineWidth = 8;
                canvas.lineCap = "round";

                let index;
                if(this.smoothSecondPointer) {
                    index = ((currentTimestamp.getHours() % 12) * 3600 + currentTimestamp.getMinutes() * 60 + currentTimestamp.getSeconds()) / (6 * 60 * 60) * Math.PI;
                } else {
                    index = currentTimestamp.getHours() % 12 / 6 * Math.PI;
                }
                const x = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;
                canvas.moveTo(properties.width / 2, properties.height / 2);
                canvas.lineTo(x, y);
                canvas.stroke();
            }
            canvas.closePath();

            canvas.beginPath();
            {
                let distance = 70;
                canvas.lineWidth = 6;
                canvas.lineCap = "round";

                let index;
                if(this.smoothSecondPointer) {
                    index = (currentTimestamp.getMinutes() * 60 + currentTimestamp.getSeconds()) / (30 * 60) * Math.PI;
                } else {
                    index = currentTimestamp.getMinutes() / 30 * Math.PI;
                }
                const x = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;
                canvas.moveTo(properties.width / 2, properties.height / 2);
                canvas.lineTo(x, y);
                canvas.stroke();
            }
            canvas.closePath();

            canvas.beginPath();
            {
                let distance = 25;
                canvas.lineWidth = 3;
                canvas.lineCap = "round";

                let index;
                if(this.smoothSecondPointer) {
                    index = (currentTimestamp.getSeconds() * 1000 + currentTimestamp.getMilliseconds()) / 30000 * Math.PI;
                } else {
                    index = currentTimestamp.getSeconds() / 30 * Math.PI;
                }
                const x = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;
                canvas.moveTo(properties.width / 2, properties.height / 2);
                canvas.lineTo(x, y);
                canvas.stroke();
            }
            canvas.closePath();

            canvas.beginPath();
            canvas.ellipse(properties.width / 2, properties.height / 2, 10, 10, 0, 0, 360);
            canvas.fill();
            canvas.closePath();
        }

        /* the brand */
        if(Math.floor(properties.timestamp / (10 * 1000)) % 2 === 0) {
            canvas.fillText("A non sober renderer", properties.width / 2, properties.height * .60);
            canvas.fillText("created by WolverinDEV", properties.width / 2, properties.height * .60 + 20);
            canvas.fillText("14/10/20 1:50 AM", properties.width / 2, properties.height * .60 + 20 * 2);
        } else {
            canvas.fillText("Time is ticking...", properties.width / 2, properties.height * .60);
            canvas.fillText("and my heart is beating", properties.width / 2, properties.height * .60 + 20);
            canvas.fillText("come time is ticking", properties.width / 2, properties.height * .60 + 40);
        }
    }

}