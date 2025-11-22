use anyhow::{Context as AnyhowContext, Result};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use clap::{Parser, ValueEnum};
use regex::Regex;
use scraper::{Html, Selector};
use serenity::all::{ChannelId, CreateEmbed, CreateMessage, Context, Ready};
use serenity::async_trait;
use serenity::prelude::*;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration as TokioDuration};

const DAILY_URL: &str = "https://wiki.guildwars.com/wiki/Daily_activities";
const WEEKLY_URL: &str = "https://wiki.guildwars.com/wiki/Weekly_activities";
const MAX_BACKOFF_SECONDS: u64 = 300; // 5 minutes
const INITIAL_BACKOFF_SECONDS: u64 = 1;

#[derive(Parser, Debug)]
#[command(name = "sheepnet")]
#[command(about = "Guild Wars daily activities Discord bot", long_about = None)]
struct Args {
    /// Run in loop mode (keep running daily) or run once
    #[arg(long, default_value_t = false)]
    r#loop: bool,

    /// Run immediately instead of waiting until 16:00 UTC
    #[arg(long, default_value_t = false)]
    now: bool,

    /// Discord channel ID (overrides CHANNEL_ID environment variable)
    #[arg(long)]
    discord_channel_id: Option<u64>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Discord)]
    output_format: OutputFormat,

    /// Simulate a specific time (format: YYYY-MM-DDTHH:MM:SS, e.g., 2025-11-25T17:00:00)
    #[arg(long)]
    at_time: Option<String>,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    /// Post to Discord
    Discord,
    /// Plain text output
    Txt,
    /// Markdown output
    Md,
    /// HTML output
    Html,
}

struct Handler {
    channel_id: ChannelId,
    http_client: reqwest::Client,
    run_once: bool,
    started: Arc<AtomicBool>,
    post_now: bool,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);

        // Prevent spawning multiple timers on reconnect
        if self.started.swap(true, Ordering::SeqCst) {
            println!("Reconnected, but timer already running");
            return;
        }

        let ctx = Arc::new(ctx);
        let channel_id = self.channel_id;
        let http_client = self.http_client.clone();
        let run_once = self.run_once;
        let post_now = self.post_now;

        tokio::spawn(async move {
            loop {
                if !post_now {
                    let now = Utc::now();
                    let target_time = get_target_time(&now);
                    let delay = (target_time - now).num_seconds().max(0) as u64;
                    println!("Sleeping {} seconds until next post", delay);
                    sleep(TokioDuration::from_secs(delay)).await;
                }

                if let Err(e) = daily_post(&ctx, channel_id, &http_client).await {
                    eprintln!("Error in daily post: {}", e);
                }

                if run_once {
                    println!("Single run completed, exiting...");
                    std::process::exit(0);
                }

                // After first (immediate) post, wait for next scheduled time
                if post_now {
                    let now = Utc::now();
                    let target_time = get_target_time(&now);
                    let delay = (target_time - now).num_seconds().max(0) as u64;
                    println!("Sleeping {} seconds until next post", delay);
                    sleep(TokioDuration::from_secs(delay)).await;
                }
            }
        });
    }
}

fn get_target_time(now: &DateTime<Utc>) -> DateTime<Utc> {
    let mut target = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 16, 0, 5)
        .unwrap();

    if *now >= target {
        target = target + Duration::days(1);
    }

    target
}

fn get_current_daily_date(now: &DateTime<Utc>) -> DateTime<Utc> {
    // Dailies change at 16:00 UTC, but we wait until 16:00:05 to be safe
    // If current time is before 16:00:05, use yesterday's date
    let daily_cutoff = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 16, 0, 5)
        .unwrap();

    if *now < daily_cutoff {
        // Before 16:00:05 UTC - use previous day
        *now - Duration::days(1)
    } else {
        // After 16:00:05 UTC - use current day
        *now
    }
}

fn get_nicholas_sandford_date(now: &DateTime<Utc>) -> DateTime<Utc> {
    // Nicholas Sandford changes at 07:00 UTC
    // If current time is before 07:00, use yesterday's date
    let ns_cutoff = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 7, 0, 0)
        .unwrap();

    if *now < ns_cutoff {
        // Before 07:00 UTC - use previous day
        *now - Duration::days(1)
    } else {
        // After 07:00 UTC - use current day
        *now
    }
}

fn get_current_weekly_date(now: &DateTime<Utc>) -> DateTime<Utc> {
    // Weekly activities change at 15:00 UTC on Mondays
    // If current time is before 15:00, use the previous weekly period
    let weekly_cutoff = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 15, 0, 0)
        .unwrap();

    let base_time = Utc
        .with_ymd_and_hms(2025, 2, 10, 15, 0, 0)
        .unwrap()
        .timestamp();
    let current_time = if *now < weekly_cutoff {
        (*now - Duration::days(1)).timestamp()
    } else {
        now.timestamp()
    };
    let one_week = 3600 * 24 * 7;

    let mut target_time = base_time;
    while (target_time + one_week) < current_time {
        target_time += one_week;
    }

    DateTime::from_timestamp(target_time, 0).unwrap()
}

async fn fetch_with_retry(http_client: &reqwest::Client, url: &str, label: &str) -> Result<String> {
    let mut backoff = INITIAL_BACKOFF_SECONDS;

    loop {
        match http_client.get(url).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    match response.text().await {
                        Ok(body) => return Ok(body),
                        Err(e) => {
                            eprintln!("Failed to read {} response body: {}", label, e);
                        }
                    }
                } else {
                    eprintln!("{} returned HTTP {} - retrying in {}s", label, status, backoff);
                }
            }
            Err(e) => {
                eprintln!("Failed to fetch {}: {} - retrying in {}s", label, e, backoff);
            }
        }

        sleep(TokioDuration::from_secs(backoff)).await;

        backoff = (backoff * 2).min(MAX_BACKOFF_SECONDS);
    }
}

async fn daily_post(ctx: &Context, channel_id: ChannelId, http_client: &reqwest::Client) -> Result<()> {
    println!("Tick");

    let now = Utc::now();
    let daily_date = get_current_daily_date(&now);
    let ns_date = get_nicholas_sandford_date(&now);

    let daily_body = fetch_with_retry(http_client, DAILY_URL, "Daily activities").await?;
    let daily_data = get_daily_data(&daily_body, &daily_date, &ns_date)?;

    let weekly_body = fetch_with_retry(http_client, WEEKLY_URL, "Weekly activities").await?;
    let weekly_data = get_weekly_data(&weekly_body, &now)?;

    let message = create_daily_message(daily_data, weekly_data, &daily_date);

    channel_id
        .send_message(&ctx.http, message)
        .await
        .with_context(|| "Failed to send message")?;

    Ok(())
}

async fn fetch_and_format(
    http_client: &reqwest::Client,
    format: &OutputFormat,
    now: &DateTime<Utc>,
) -> Result<String> {
    let daily_date = get_current_daily_date(now);
    let ns_date = get_nicholas_sandford_date(now);

    let daily_body = fetch_with_retry(http_client, DAILY_URL, "Daily activities").await?;
    let daily_data = get_daily_data(&daily_body, &daily_date, &ns_date)?;

    let weekly_body = fetch_with_retry(http_client, WEEKLY_URL, "Weekly activities").await?;
    let weekly_data = get_weekly_data(&weekly_body, now)?;

    Ok(format_output(&daily_data, &weekly_data, &daily_date, format))
}

#[derive(Debug)]
pub struct DailyData {
    pub ns: String,
    pub vq: String,
    pub sb: String,
    pub zm: String,
    pub zb: String,
    pub zc: String,
    pub zv: String,
}

#[derive(Debug)]
pub struct WeeklyData {
    pub ni: String,
    pub pve: String,
    pub pvp: String,
}

pub fn get_daily_data(body: &str, daily_date: &DateTime<Utc>, ns_date: &DateTime<Utc>) -> Result<DailyData> {
    let daily_search = daily_date.format("%-d %B %Y").to_string();
    let ns_search = ns_date.format("%-d %B %Y").to_string();
    let document = Html::parse_document(body);
    let tbody_selector = Selector::parse("div.mw-parser-output table tbody").unwrap();
    let tr_selector = Selector::parse("tr").unwrap();

    let tbody = document
        .select(&tbody_selector)
        .next()
        .with_context(|| "Could not find table tbody")?;

    let mut daily_found = false;
    let mut daily_data = DailyData {
        zm: String::new(),
        zb: String::new(),
        zc: String::new(),
        zv: String::new(),
        sb: String::new(),
        vq: String::new(),
        ns: String::new(),
    };

    // First pass: get regular dailies (16:00 UTC)
    for tr in tbody.select(&tr_selector) {
        let cells: Vec<_> = tr.child_elements().collect();
        if cells.len() < 8 {
            continue;
        }

        let date_text = cells[0].text().collect::<String>();
        if date_text.trim() == daily_search {
            daily_data.zm = convert_link(&get_html(&cells[1]))?;
            daily_data.zb = convert_link(&get_html(&cells[2]))?;
            daily_data.zc = convert_link(&get_html(&cells[3]))?;
            daily_data.zv = convert_link(&get_html(&cells[4]))?;
            daily_data.sb = convert_link(&get_html(&cells[5]))?;
            daily_data.vq = convert_link(&get_html(&cells[6]))?;
            daily_found = true;
            break;
        }
    }

    if !daily_found {
        return Err(anyhow::anyhow!("No daily data found for {}", daily_search));
    }

    // Second pass: get Nicholas Sandford (07:00 UTC)
    let mut ns_found = false;
    for tr in tbody.select(&tr_selector) {
        let cells: Vec<_> = tr.child_elements().collect();
        if cells.len() < 8 {
            continue;
        }

        let date_text = cells[0].text().collect::<String>();
        if date_text.trim() == ns_search {
            daily_data.ns = convert_link(&get_html(&cells[7]))?;
            ns_found = true;
            break;
        }
    }

    if !ns_found {
        return Err(anyhow::anyhow!("No Nicholas Sandford data found for {}", ns_search));
    }

    Ok(daily_data)
}

pub fn get_weekly_data(body: &str, now: &DateTime<Utc>) -> Result<WeeklyData> {
    let weekly_date = get_current_weekly_date(now);
    let search = weekly_date.format("%-d %B %Y").to_string();
    let document = Html::parse_document(body);
    let tbody_selector = Selector::parse("div.mw-parser-output table tbody").unwrap();
    let tr_selector = Selector::parse("tr").unwrap();

    let tbody = document
        .select(&tbody_selector)
        .next()
        .with_context(|| "Could not find table tbody")?;

    for tr in tbody.select(&tr_selector) {
        let cells: Vec<_> = tr.child_elements().collect();
        if cells.len() < 5 {
            continue;
        }

        let date_text = cells[0].text().collect::<String>();
        if date_text.trim() != search {
            continue;
        }

        return Ok(WeeklyData {
            pve: strip_link(&get_html(&cells[1]))?,
            pvp: strip_link(&get_html(&cells[2]))?,
            ni: convert_link(&get_html(&cells[3]))?,
        });
    }

    Err(anyhow::anyhow!("No weekly data found for {}", search))
}

pub fn convert_link(html: &str) -> Result<String> {
    // Match <a> tags with href attribute (in any position)
    let link_re = Regex::new(r#"<a\s+[^>]*href="([^"]+)"[^>]*>(.+?)</a>"#).unwrap();
    if let Some(caps) = link_re.captures(html) {
        let url = &caps[1];
        let text = &caps[2];
        let url_escaped = url.replace(')', "%29");
        
        // Extract any text after the link (e.g., " (3x)")
        let after_link = html[caps.get(0).unwrap().end()..].trim();
        
        if after_link.is_empty() {
            return Ok(format!("[{}](https://wiki.guildwars.com{})", text, url_escaped));
        } else {
            // Remove remaining HTML tags from the suffix
            let html_tag_re = Regex::new(r"<[^>]+>").unwrap();
            let clean_suffix = html_tag_re.replace_all(after_link, "");
            return Ok(format!("[{}](https://wiki.guildwars.com{}) {}", text, url_escaped, clean_suffix));
        }
    }

    let html_tag_re = Regex::new(r"<[^>]+>").unwrap();
    let stripped = html_tag_re.replace_all(html, "").to_string();

    Ok(stripped)
}

pub fn strip_link(html: &str) -> Result<String> {
    // Extract text from <a> tag without creating a link
    let link_re = Regex::new(r#"<a\s+[^>]*>(.+?)</a>"#).unwrap();
    if let Some(caps) = link_re.captures(html) {
        let text = caps[1].to_string();
        
        // Extract any text after the link (e.g., " (3x)")
        let after_link = html[caps.get(0).unwrap().end()..].trim();
        
        if after_link.is_empty() {
            return Ok(text);
        } else {
            // Remove remaining HTML tags from the suffix
            let html_tag_re = Regex::new(r"<[^>]+>").unwrap();
            let clean_suffix = html_tag_re.replace_all(after_link, "");
            return Ok(format!("{} {}", text, clean_suffix));
        }
    }

    // Fallback: strip all HTML tags
    let html_tag_re = Regex::new(r"<[^>]+>").unwrap();
    let stripped = html_tag_re.replace_all(html, "").to_string();

    Ok(stripped)
}

fn get_html(element: &scraper::ElementRef) -> String {
    element.inner_html().trim().to_string()
}

fn strip_markdown_links(text: &str) -> String {
    let re = Regex::new(r"\[(.+?)\]\((.+?)\)").unwrap();
    let stripped = re.replace_all(text, "$1").to_string();
    let html_re = Regex::new(r"<[^>]+>").unwrap();
    html_re.replace_all(&stripped, "").to_string()
}

fn markdown_to_html_links(text: &str) -> String {
    let re = Regex::new(r"\[(.+?)\]\((.+?)\)").unwrap();
    re.replace_all(text, r#"<a href="$2">$1</a>"#).to_string()
}

fn format_output(daily: &DailyData, weekly: &WeeklyData, now: &DateTime<Utc>, format: &OutputFormat) -> String {
    let date_str = now.format("%-d %B %Y").to_string();

    match format {
        OutputFormat::Txt => {
            format!(
                "Dailies for {}\n\
                 \n\
                 Nicholas Sandford.....: {}\n\
                 Vanguard Quest........: {}\n\
                 Wanted................: {}\n\
                 \n\
                 Zaishen Mission.......: {}\n\
                 Zaishen Bounty........: {}\n\
                 Zaishen Combat........: {}\n\
                 Zaishen Vanquish......: {}\n\
                 \n\
                 Weekly bonuses:\n\
                 Nicholas the Traveller: {}\n\
                 PvE Bonus.............: {}\n\
                 PvP Bonus.............: {}",
                date_str,
                strip_markdown_links(&daily.ns),
                strip_markdown_links(&daily.vq),
                strip_markdown_links(&daily.sb),
                strip_markdown_links(&daily.zm),
                strip_markdown_links(&daily.zb),
                strip_markdown_links(&daily.zc),
                strip_markdown_links(&daily.zv),
                strip_markdown_links(&weekly.ni),
                strip_markdown_links(&weekly.pve),
                strip_markdown_links(&weekly.pvp)
            )
        }
        OutputFormat::Md => {
            format!(
                "# Dailies for {}\n\
                 \n\
                 - **Nicholas Sandford**: {}\n\
                 - **Vanguard Quest**: {}\n\
                 - **Wanted**: {}\n\
                 \n\
                 ## Zaishen Quests\n\
                 \n\
                 - **Zaishen Mission**: {}\n\
                 - **Zaishen Bounty**: {}\n\
                 - **Zaishen Combat**: {}\n\
                 - **Zaishen Vanquish**: {}\n\
                 \n\
                 ## Weekly bonuses\n\
                 \n\
                 - **Nicholas the Traveller**: {}\n\
                 - **PvE Bonus**: {}\n\
                 - **PvP Bonus**: {}",
                date_str,
                daily.ns,
                daily.vq,
                daily.sb,
                daily.zm,
                daily.zb,
                daily.zc,
                daily.zv,
                weekly.ni,
                weekly.pve,
                weekly.pvp
            )
        }
        OutputFormat::Html => {
            format!(
                "<!DOCTYPE html>\n\
                 <html>\n\
                 <head>\n\
                     <meta charset=\"utf-8\">\n\
                     <title>Dailies for {}</title>\n\
                     <style>\n\
                         body {{ font-family: Arial, sans-serif; max-width: 800px; margin: 20px auto; padding: 20px; }}\n\
                         h1 {{ color: #2c3e50; }}\n\
                         h2 {{ color: #34495e; margin-top: 30px; }}\n\
                         .activity {{ margin: 10px 0; padding: 8px; background: #ecf0f1; border-radius: 4px; }}\n\
                         .label {{ font-weight: bold; display: inline-block; width: 200px; }}\n\
                         a {{ color: #3498db; text-decoration: none; }}\n\
                         a:hover {{ text-decoration: underline; }}\n\
                     </style>\n\
                 </head>\n\
                 <body>\n\
                     <h1>Dailies for {}</h1>\n\
                     <div class=\"activity\"><span class=\"label\">Nicholas Sandford:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">Vanguard Quest:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">Wanted:</span> {}</div>\n\
                     <h2>Zaishen Quests</h2>\n\
                     <div class=\"activity\"><span class=\"label\">Zaishen Mission:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">Zaishen Bounty:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">Zaishen Combat:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">Zaishen Vanquish:</span> {}</div>\n\
                     <h2>Weekly bonuses</h2>\n\
                     <div class=\"activity\"><span class=\"label\">Nicholas the Traveller:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">PvE Bonus:</span> {}</div>\n\
                     <div class=\"activity\"><span class=\"label\">PvP Bonus:</span> {}</div>\n\
                 </body>\n\
                 </html>",
                date_str, date_str,
                markdown_to_html_links(&daily.ns),
                markdown_to_html_links(&daily.vq),
                markdown_to_html_links(&daily.sb),
                markdown_to_html_links(&daily.zm),
                markdown_to_html_links(&daily.zb),
                markdown_to_html_links(&daily.zc),
                markdown_to_html_links(&daily.zv),
                markdown_to_html_links(&weekly.ni),
                markdown_to_html_links(&weekly.pve),
                markdown_to_html_links(&weekly.pvp)
            )
        }
        OutputFormat::Discord => {
            format!(
                "`Nicholas Sandford.....`: {}\n\
                 `Vanguard Quest........`: {}\n\
                 `Wanted................`: {}\n\
                 \n\
                 `Zaishen Mission.......`: {}\n\
                 `Zaishen Bounty........`: {}\n\
                 `Zaishen Combat........`: {}\n\
                 `Zaishen Vanquish......`: {}\n\
                 \n\
                 **Weekly bonuses:**\n\
                 `Nicholas the Traveller`: {}\n\
                 `PvE Bonus.............`: {}\n\
                 `PvP Bonus.............`: {}",
                daily.ns,
                daily.vq,
                daily.sb,
                daily.zm,
                daily.zb,
                daily.zc,
                daily.zv,
                weekly.ni,
                weekly.pve,
                weekly.pvp
            )
        }
    }
}

fn create_daily_message(daily: DailyData, weekly: WeeklyData, now: &DateTime<Utc>) -> CreateMessage {
    let title = format!("Dailies for {}", now.format("%-d %B %Y"));
    let description = format_output(&daily, &weekly, now, &OutputFormat::Discord);

    let embed = CreateEmbed::new().title(title).description(description);

    CreateMessage::new().embed(embed)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Parse the simulated time if provided
    let now = if let Some(ref time_str) = args.at_time {
        chrono::NaiveDateTime::parse_from_str(time_str, "%Y-%m-%dT%H:%M:%S")
            .with_context(|| format!("Invalid time format: {}. Use YYYY-MM-DDTHH:MM:SS", time_str))?
            .and_utc()
    } else {
        Utc::now()
    };

    if args.at_time.is_some() {
        println!("Simulating time: {}", now.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    let http_client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; GuildWarsBot/1.0)")
        .build()
        .with_context(|| "Failed to create HTTP client")?;

    if !matches!(args.output_format, OutputFormat::Discord) {
        if !args.now && args.at_time.is_none() {
            let target_time = get_target_time(&now);
            let delay = (target_time - now).num_seconds().max(0) as u64;
            println!("Waiting {} seconds until 16:00 UTC...", delay);
            sleep(TokioDuration::from_secs(delay)).await;
        }

        loop {
            match fetch_and_format(&http_client, &args.output_format, &now).await {
                Ok(output) => println!("{}", output),
                Err(e) => eprintln!("Error: {}", e),
            }

            if !args.r#loop {
                break;
            }

            if args.at_time.is_some() {
                println!("Note: --at-time is set, loop mode doesn't make sense with simulated time");
                break;
            }

            let current_now = Utc::now();
            let target_time = get_target_time(&current_now);
            let delay = (target_time - current_now).num_seconds().max(0) as u64;
            println!("\nWaiting {} seconds until next update...", delay);
            sleep(TokioDuration::from_secs(delay)).await;
        }

        return Ok(());
    }

    // Discord mode not supported with --at-time
    if args.at_time.is_some() {
        anyhow::bail!("--at-time is not supported with Discord output format. Use --output-format txt/md/html instead.");
    }

    let token = env::var("TOKEN").with_context(|| "TOKEN environment variable not set")?;

    let channel_id = if let Some(id) = args.discord_channel_id {
        id
    } else {
        let channel_id_str = env::var("CHANNEL_ID").with_context(|| "CHANNEL_ID environment variable not set")?;
        channel_id_str.parse().with_context(|| "CHANNEL_ID must be a valid number")?
    };

    let intents = GatewayIntents::empty();

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler {
            channel_id: ChannelId::new(channel_id),
            http_client,
            run_once: !args.r#loop,
            started: Arc::new(AtomicBool::new(false)),
            post_now: args.now,
        })
        .await
        .with_context(|| "Failed to create Discord client")?;

    if args.now {
        client.start().await.with_context(|| "Client error")?;
    } else {
        client.start().await.with_context(|| "Client error")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAILY_HTML: &str = include_str!("../tests/fixtures/daily_activities.html");
    const WEEKLY_HTML: &str = include_str!("../tests/fixtures/weekly_activities.html");

    #[test]
    fn test_parse_daily_data() {
        let test_date = Utc.with_ymd_and_hms(2025, 11, 22, 16, 0, 0).unwrap();
        let test_ns_date = Utc.with_ymd_and_hms(2025, 11, 22, 7, 0, 0).unwrap();
        let result = get_daily_data(DAILY_HTML, &test_date, &test_ns_date);

        assert!(result.is_ok(), "Failed to parse daily data: {:?}", result.err());

        let data = result.unwrap();
        assert!(!data.zm.is_empty(), "Zaishen Mission should not be empty");
        assert!(!data.zb.is_empty(), "Zaishen Bounty should not be empty");
        assert!(!data.zc.is_empty(), "Zaishen Combat should not be empty");
        assert!(!data.zv.is_empty(), "Zaishen Vanquish should not be empty");
        assert!(!data.sb.is_empty(), "Wanted should not be empty");
        assert!(!data.vq.is_empty(), "Vanguard Quest should not be empty");
        assert!(!data.ns.is_empty(), "Nicholas Sandford should not be empty");
        
        // Check that links are present
        assert!(data.zm.contains("]("), "Zaishen Mission should have a link: {}", data.zm);
        assert!(data.zb.contains("]("), "Zaishen Bounty should have a link: {}", data.zb);
        assert!(data.zc.contains("]("), "Zaishen Combat should have a link: {}", data.zc);
        assert!(data.zv.contains("]("), "Zaishen Vanquish should have a link: {}", data.zv);
    }

    #[test]
    fn test_parse_weekly_data() {
        let test_date = Utc.with_ymd_and_hms(2025, 11, 17, 16, 0, 0).unwrap();
        let result = get_weekly_data(WEEKLY_HTML, &test_date);

        assert!(result.is_ok(), "Failed to parse weekly data: {:?}", result.err());

        let data = result.unwrap();
        assert!(!data.pve.is_empty(), "PvE Bonus should not be empty");
        assert!(!data.pvp.is_empty(), "PvP Bonus should not be empty");
        assert!(!data.ni.is_empty(), "Nicholas the Traveller should not be empty");
    }

    #[test]
    fn test_convert_link() {
        let html = r#"<a href="/wiki/Test_Page">Test Link</a>"#;
        let result = convert_link(html).unwrap();
        assert_eq!(result, "[Test Link](https://wiki.guildwars.com/wiki/Test_Page)");

        let html_with_paren = r#"<a href="/wiki/Test_(Page)">Test Link</a>"#;
        let result = convert_link(html_with_paren).unwrap();
        assert_eq!(result, "[Test Link](https://wiki.guildwars.com/wiki/Test_(Page%29)");

        let plain_text = "Plain text";
        let result = convert_link(plain_text).unwrap();
        assert_eq!(result, "Plain text");
    }

    #[test]
    fn test_strip_markdown_links() {
        let text = "[Test Link](https://example.com)";
        assert_eq!(strip_markdown_links(text), "Test Link");

        let text = "Before [Link](url) After";
        assert_eq!(strip_markdown_links(text), "Before Link After");
    }

    #[test]
    fn test_markdown_to_html_links() {
        let text = "[Test](https://example.com)";
        assert_eq!(markdown_to_html_links(text), r#"<a href="https://example.com">Test</a>"#);
    }

    #[test]
    fn test_format_output_txt() {
        let daily = DailyData {
            ns: "Test NS".to_string(),
            vq: "Test VQ".to_string(),
            sb: "Test Wanted".to_string(),
            zm: "Test ZM".to_string(),
            zb: "Test ZB".to_string(),
            zc: "Test ZC".to_string(),
            zv: "Test ZV".to_string(),
        };

        let weekly = WeeklyData {
            ni: "Test NI".to_string(),
            pve: "Test PvE".to_string(),
            pvp: "Test PvP".to_string(),
        };

        let now = Utc.with_ymd_and_hms(2024, 11, 22, 16, 0, 0).unwrap();
        let output = format_output(&daily, &weekly, &now, &OutputFormat::Txt);

        assert!(output.contains("Dailies for 22 November 2024"));
        assert!(output.contains("Test NS"));
        assert!(output.contains("Test VQ"));
    }
}
