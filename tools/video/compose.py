# compose.py spec.json out.mp4 : true-flow video. Each section = a source range [a,b] (recording seconds),
# a mode ("hyd" = Hydrogen chat full frame, "fus" = Fusion full frame with the live chat inset), a short
# label and one narration line. Each section's picture is retimed to fit its line, so there is no dead air.
import json, subprocess, sys, os
spec, out = json.load(open(sys.argv[1])), sys.argv[2]
F, H, CROP = spec["fusion"], spec["hydrogen"], spec.get("chatCrop", "1192:1250:88:268")
ICROP = spec.get("insetCrop", "1192:760:88:758")
LEAD, GAP, MINF = 0.1, 0.3, 0.5
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
def dur(p): return float(subprocess.run(["ffprobe","-v","error","-show_entries","format=duration","-of","csv=p=0",p],capture_output=True,text=True).stdout)
def esc(t): return t.replace("\\","\\\\").replace(":", "\\:").replace("'", "’")
stem = os.path.splitext(os.path.basename(out))[0]; os.makedirs("cmp", exist_ok=True)
parts, t, rep = [], 0.0, []
for i, s in enumerate(spec["sections"]):
    a, b = s.get("a",0), s.get("b",1); mp3 = f"cmp/{stem}-{i}.mp3"; raw = mp3.replace(".mp3", "-raw.mp3")
    subprocess.run(["adom-tts","say","--out",raw,"--rate","+4%",s["text"]], check=True, capture_output=True)
    subprocess.run(["ffmpeg","-nostdin","-v","error","-y","-i",raw,"-af","silenceremove=start_periods=1:start_threshold=-45dB,areverse,silenceremove=start_periods=1:start_threshold=-45dB,areverse","-c:a","libmp3lame","-q:a","2",mp3], check=True)
    d = dur(mp3); f = max((b - a) / (LEAD + d + GAP), MINF)
    lab = f",drawtext=fontfile={FONT}:text='{esc(s['label'])}':x=28:y=26:fontsize=30:fontcolor=white:box=1:boxcolor=0x0d1117@0.85:boxborderw=12"
    if s["mode"] == "img":
        seg = f"cmp/{stem}-v{i}.mp4"; L = LEAD + d + GAP
        subprocess.run(["nice","-n","19","ffmpeg","-nostdin","-v","error","-y","-threads","2","-loop","1","-t",f"{L:.3f}","-i",s["img"],
            "-vf","scale=1920:1080,fps=30,format=yuv420p","-c:v","libx264","-x264-params","threads=2","-crf","20","-preset","fast","-g","30","-keyint_min","30",seg], check=True)
        sd = dur(seg); parts.append((seg, mp3, t + LEAD)); rep.append(f"{i}: card {s['img']} -> {t:5.1f}-{t+sd:5.1f}"); t += sd; continue
    if s["mode"] == "full":
        fc = f"[0:v]crop={spec.get('fullCrop','iw:ih:0:0')},scale=-2:1080,pad=1920:1080:(ow-iw)/2:0:color=0x0d1117,setpts=(PTS-STARTPTS)/{f},fps=30{lab}[v]"
    elif s["mode"] == "hyd":
        fc = f"[1:v]crop={CROP},scale=-2:1080,pad=1920:1080:(ow-iw)/2:0:color=0x0d1117,setpts=(PTS-STARTPTS)/{f},fps=30{lab}[v]"
    else:
        fc = (f"[0:v]scale=1920:-2,pad=1920:1080:0:(oh-ih)/2:color=0x0d1117,setpts=(PTS-STARTPTS)/{f}[bg];"
              f"[1:v]crop={ICROP},scale=760:-2,pad=iw+6:ih+6:3:3:color=0x00b8b1,setpts=(PTS-STARTPTS)/{f}[ins];"
              f"[bg][ins]overlay=W-w-24:H-h-24:shortest=1,fps=30{lab}[v]")
    seg = f"cmp/{stem}-v{i}.mp4"
    subprocess.run(["nice","-n","19","ffmpeg","-nostdin","-v","error","-y","-threads","2",
        "-ss",str(a),"-t",str(b-a),"-i",F,"-ss",str(a),"-t",str(b-a),"-i",H,"-filter_complex",fc,"-map","[v]","-an",
        "-c:v","libx264","-x264-params","threads=2","-crf","21","-preset","fast","-g","30","-keyint_min","30","-pix_fmt","yuv420p",seg], check=True)
    sd = dur(seg); parts.append((seg, mp3, t + LEAD)); rep.append(f"{i}: {a:6.1f}-{b:6.1f} {s['mode']} x{f:5.2f} -> {t:5.1f}-{t+sd:5.1f} line {d:4.1f}s"); t += sd
open(f"cmp/{stem}.txt","w").write("".join(f"file '{os.path.basename(p)}'\n" for p,_,_ in parts))
subprocess.run(["ffmpeg","-nostdin","-v","error","-y","-f","concat","-safe","0","-i",f"cmp/{stem}.txt","-c","copy",f"cmp/{stem}-video.mp4"], check=True)
ins, flt = [], []
for k,(_,mp3,at) in enumerate(parts): ins += ["-i", mp3]; flt.append(f"[{k+1}:a]adelay={int(at*1000)}:all=1[a{k}]")
fc = ";".join(flt)+";"+"".join(f"[a{k}]" for k in range(len(parts)))+f"amix=inputs={len(parts)}:normalize=0,apad[aout]"
subprocess.run(["nice","-n","19","ffmpeg","-nostdin","-v","error","-y","-i",f"cmp/{stem}-video.mp4",*ins,"-filter_complex",fc,"-map","0:v","-map","[aout]","-c:v","copy","-c:a","aac","-b:a","160k","-shortest","-movflags","+faststart",out], check=True)
print("\n".join(rep)); print(f"total {dur(out):.1f}s")
