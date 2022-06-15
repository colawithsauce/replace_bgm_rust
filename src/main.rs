use ffmpeg_next as ffmpeg;
use itertools::Itertools;
use rand::{seq::SliceRandom, thread_rng};
use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::Command,
};
use tempfile::NamedTempFile;
use which::which;

// 生成BGM的一条规则
struct RuleEntry {
    url: String,
    duration: i64, // 单位为分钟
    audio_files: Vec<String>,
}

impl RuleEntry {
    // 获取随机的一首歌
    fn get_random_audio(&self) -> String {
        self.audio_files.choose(&mut thread_rng()).unwrap().clone()
    }

    fn get_duration_in_seconds(&self) -> f64 {
        self.duration as f64 * 60.0
    }
}

struct DisplayName {
    name: String,
    start: f64, // In seconds
    end: f64,   // In seconds
}

// 使用 ffprobe 获取多媒体文件的时长
fn get_duration(path: &str) -> f64 {
    // 检测系统的 PATH 中是否存在 ffprobe 可执行文件
    let ffprobe_path = which("ffprobe").unwrap();
    let ffprobe_path = ffprobe_path.to_str().unwrap();

    // 使用 ffprobe 获取音频文件的时长
    let output = Command::new(ffprobe_path)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .output()
        .expect("请检查 ffprobe 是否存在！");

    // 返回音频文件的时长
    return String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .unwrap();
}

// 读取脚本，并填充规则文件。并且给每个文件的url加上script文件所在目录。
fn read_script(url: &str, rule_entries: &mut Vec<RuleEntry>) {
    let file = File::open(url).expect("无法打开文件");
    let buf = BufReader::new(file);
    let script_str = buf
        .lines()
        .map(|x| x.unwrap()) // 如果这里出错，直接让程序崩溃也无不可
        .filter(|x| !x.starts_with('#'))
        .tuples(); // itertools 根据使用时的情况决定几个一 tuple

    for (i, j) in script_str {
        // 第一个是目录名

        let entry = if Path::new(i.as_str()).is_absolute() {
            RuleEntry {
                url: i,
                duration: j.parse::<i64>().unwrap(),
                audio_files: vec![],
            }
        } else {
            // 这是 script.txt 所在目录相对于当前目录的偏移。
            let dir = Path::new(url).parent().unwrap();
            let dir = if dir.is_absolute() {
                dir.to_str().unwrap().to_string()
            } else {
                // 如果是相对路径，那么就要加上当前目录
                let current_dir = env::current_dir().unwrap();
                format!("{}{}", current_dir.to_str().unwrap(), dir.to_str().unwrap()) // 这里不需要加斜杠，因为 current_dir 已经加了
            };
            RuleEntry {
                // 在 url 所指向文件的所在目录下面寻找 Entry
                url: format!("{}/{}", dir, i),
                duration: j.parse::<i64>().unwrap(),
                audio_files: vec![],
            }
        };
        rule_entries.push(entry);
    }
}

// 对 rule_entries 中的每个 rule_entry，判断它是目录还是文件
// 如果是文件，则直接添加到播放列表
// 如果是目录，则遍历目录下的所有文件，并添加到播放列表
// 添加到播放列表的时候，需要把文件名加上目录名。
fn open_audio_files(rule_entries: &mut Vec<RuleEntry>) {
    for rule_entry in rule_entries.iter_mut() {
        let path = Path::new(&rule_entry.url);
        if path.is_file() {
            rule_entry.audio_files.push(rule_entry.url.clone());
        } else if path.is_dir() {
            for entry in path.read_dir().unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_file() {
                    let file_name = path.file_name().unwrap().to_str().unwrap();
                    let file_name = format!("{}/{}", rule_entry.url, file_name);
                    rule_entry.audio_files.push(file_name);
                }
            }
        }
    }
}

// 获取音频文件的显示名，将结果缓存到一个 static 的 HashMap 中，如果能够在此找到，则直接返回，
// 如果不能够找到则使用 ffprobe 来获取其名字与艺术家，缓存并返回格式 artist - title。
// 如果 ffprobe 失败，则使用文件名来作为显示名，缓存并返回
fn get_display_name(path: &str, path_displayname_map: &mut HashMap<String, String>) -> String {
    // 如果已经在 HashMap 中，则直接返回
    if let Some(display_name) = path_displayname_map.get(path) {
        return display_name.clone();
    }

    // 默认显示名是文件的 filename
    let default_name = path.split('/').last().unwrap().to_string();

    // 如果不在 HashMap 中，则使用 ffmpeg 找到它的 title 与 artist
    ffmpeg::init().unwrap();
    let display_name = if let Ok(format) = ffmpeg::format::input(&path) {
        let mut display_name = String::new();
        if let Some(artist) = format.metadata().get("artist") {
            display_name.push_str(artist);
            display_name.push_str(" - ");
        }
        if let Some(title) = format.metadata().get("title") {
            display_name.push_str(title);
        }
        if display_name.is_empty() {
            display_name = default_name;
        }
        display_name
    } else {
        panic!("无法打开文件：{}\n", path);
    };

    // 加入 HashMap，并且返回
    path_displayname_map.insert(path.to_string(), display_name.clone());
    return display_name;
}

// 根据规则，随机地生成播放列表
// total_duration 的单位是秒
fn generate_play_list(
    rule_entries: &mut Vec<RuleEntry>,
    total_duration: f64,
) -> (Vec<String>, Vec<DisplayName>) {
    let mut play_list: Vec<String> = vec![];
    let mut display_namelist: Vec<DisplayName> = vec![];
    let mut path_displayname_map: HashMap<String, String> = HashMap::new();
    let mut total_time = 0f64; // 总的 duration
    let mut cur_time = 0f64; // 在当前目录中的 duration
    let mut index = 0;

    while total_time < total_duration {
        let rule_entry = &mut rule_entries[index];
        let audio = rule_entry.get_random_audio();
        let prev_time = total_time; // 此文件的开始时间

        play_list.push(audio.clone());
        let audio_duration = get_duration(&audio);
        total_time += audio_duration; // 此文件的结束时间
        cur_time += audio_duration;

        display_namelist.push(DisplayName {
            name: get_display_name(&audio, &mut path_displayname_map),
            start: prev_time,
            end: total_time,
        });

        if cur_time > rule_entry.get_duration_in_seconds() {
            cur_time = 0.0;
            index = (index + 1) % rule_entries.len();
        }
    }

    (play_list, display_namelist)
}

// 根据 display_namelist 生成 srt 临时字幕字符串。
// 其中，name 是字幕，start 是按秒表示的开始时间，end 是按秒表示的结束时间。
// 需要将 start 与 end 的
fn generate_srt(display_namelist: &Vec<DisplayName>) -> String {
    let mut srt = String::new();
    for (i, display_name) in display_namelist.iter().enumerate() {
        let start = display_name.start.to_string();
        let end = display_name.end.to_string();
        let start_time = format_time(&start);
        let end_time = format_time(&end);
        srt.push_str(&format!("{}\n", i + 1));
        srt.push_str(&format!("{} --> {}\n", start_time, end_time));
        srt.push_str(&format!("{}\n", display_name.name));
        srt.push_str("\n");
    }
    srt
}

// 从 timestamp 转化成 Hours:Minutes:Seconds,Milliseconds 的时间表示方式
fn format_time(timestamp: &str) -> String {
    let time = timestamp.parse::<f64>().unwrap();
    let hours = (time / 3600.0).floor() as u32;
    let minutes = ((time - hours as f64 * 3600.0) / 60.0).floor() as u32;
    let seconds = (time - hours as f64 * 3600.0 - minutes as f64 * 60.0).floor() as u32;
    let milliseconds =
        (time - hours as f64 * 3600.0 - minutes as f64 * 60.0 - seconds as f64).floor() as u32;
    format!(
        "{:02}:{:02}:{:02},{:04}",
        hours, minutes, seconds, milliseconds
    )
}

// 使用 ffmpeg 的二进制文件，将 video 文件的音轨使用 playlist 中的歌曲顺序替换
// 并且保存到新的文件 output 中。
fn replace_audio(
    playlist: Vec<String>,
    video_name: String,
    display_namelist: Vec<DisplayName>,
    output: String,
) {
    let ffmpeg = which("ffmpeg").unwrap();
    let ffmpeg = ffmpeg.to_str().unwrap();

    let mut cmd = Command::new(ffmpeg);

    // 将 playlist 中的内容以 "file '{}'" 的格式保存到临时文件中
    let mut playlist_file = NamedTempFile::new().unwrap();
    for audio in playlist {
        println!("file '{}'", audio);
        writeln!(playlist_file, "file '{}'", audio).unwrap();
    }

    // 将 display_namelist 存储为 srt 文件格式到临时文件里面。
    let mut srt_file = NamedTempFile::new().unwrap();

    // 写入 srt 文件
    let srt = generate_srt(&display_namelist);
    srt_file.write_all(srt.as_bytes()).unwrap();

    // flush playlist_file 到磁盘
    playlist_file.flush().unwrap();

    // flush srt_file 到磁盘
    srt_file.flush().unwrap();

    // 建立 ffmpeg 的参数，读取之前保存的文本文件作为音频的输入
    // 使用 -i -an 参数读取 video，不进行音视频的转码，直接输出到输出文件
    // 使用 -f concat -i -vn 参数读取 playlist_file，作为音频的输入
    // 使用 -c copy 参数，将输出文件输出到输出文件
    let output = cmd
        .args(["-an", "-i"]) // 读取 video 文件
        .arg(&video_name)
        .args(["-f", "concat"])
        .args(["-safe", "0"]) // 允许使用不安全的解码器
        .arg("-i")
        .arg(&playlist_file.path().to_str().unwrap())
        .arg("-i")
        .arg(&srt_file.path().to_str().unwrap())
        .args(["-c", "copy"]) // 禁止编码转换
        .arg("-y") // 强制覆盖输出文件
        .arg(&output)
        .output()
        .expect("运行 ffmpeg 时出错");

    // Print Error Messages.
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}

fn main() {
    let args: Vec<String> = env::args().collect_vec();

    // 获取用户的输入，正确的输入格式为
    // replace_bgm script_name video_name output_name
    // 如果参数数目有误，则提示正确的输入格式
    if args.len() != 4 {
        println!("Usage: replace_bgm script_name video_name output_name");
        return;
    }

    // 获取输入的参数
    let script_name = args[1].clone();
    let video_name = args[2].clone();
    let output_name = args[3].clone();

    // 根据 script 文件生成播放列表
    let mut rules: Vec<RuleEntry> = Vec::new(); // 规则列表
    read_script(&script_name, &mut rules); // 读取规则到规则列表
    open_audio_files(&mut rules); // 打开所有的音频文件

    let (playlist, display_namelist) = generate_play_list(&mut rules, get_duration(&video_name));

    // 更换视频的背景音乐
    replace_audio(playlist, video_name, display_namelist, output_name);
}
