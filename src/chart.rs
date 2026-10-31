use std::io::Cursor;

use anyhow::{Context, Result, bail};
use plotters::prelude::*;

use crate::repository::BudgetMonth;

const WIDTH: u32 = 900;
const HEIGHT: u32 = 540;
const INCOME_COLOR: RGBColor = RGBColor(46, 160, 67);
const EXPENSE_COLOR: RGBColor = RGBColor(218, 54, 51);

pub fn render_budget_chart(months: &[BudgetMonth]) -> Result<Vec<u8>> {
    if months.is_empty() {
        bail!("cannot render an empty budget chart");
    }

    let mut pixels = vec![255_u8; (WIDTH * HEIGHT * 3) as usize];
    {
        let root = BitMapBackend::with_buffer(&mut pixels, (WIDTH, HEIGHT)).into_drawing_area();
        root.fill(&WHITE).map_err(chart_error)?;

        let maximum = months
            .iter()
            .flat_map(|month| [month.income, month.expenses])
            .max()
            .unwrap_or(0);
        let y_max = ((maximum.max(1) as f64 * 1.2).ceil() as i64).max(10);
        let labels = months
            .iter()
            .map(|month| russian_month_label(&month.label))
            .collect::<Vec<_>>();

        let mut chart = ChartBuilder::on(&root)
            .caption("Бюджет по месяцам", ("sans-serif", 38))
            .margin(30)
            .x_label_area_size(65)
            .y_label_area_size(100)
            .build_cartesian_2d(-0.5_f64..months.len() as f64 - 0.5, 0_i64..y_max)
            .map_err(chart_error)?;

        chart
            .configure_mesh()
            .disable_x_mesh()
            .light_line_style(RGBColor(225, 230, 235))
            .x_labels(months.len())
            .x_label_formatter(&|value| {
                let index = value.round();
                if index < 0.0 {
                    return String::new();
                }
                labels.get(index as usize).cloned().unwrap_or_default()
            })
            .y_label_formatter(&|value| compact_money(*value))
            .axis_desc_style(("sans-serif", 22))
            .label_style(("sans-serif", 20))
            .draw()
            .map_err(chart_error)?;

        chart
            .draw_series(months.iter().enumerate().map(|(index, month)| {
                let x = index as f64;
                Rectangle::new(
                    [(x - 0.40, 0), (x - 0.05, month.income)],
                    INCOME_COLOR.filled(),
                )
            }))
            .map_err(chart_error)?
            .label("Доходы")
            .legend(|(x, y)| Rectangle::new([(x, y - 7), (x + 24, y + 7)], INCOME_COLOR.filled()));

        chart
            .draw_series(months.iter().enumerate().map(|(index, month)| {
                let x = index as f64;
                Rectangle::new(
                    [(x + 0.05, 0), (x + 0.40, month.expenses)],
                    EXPENSE_COLOR.filled(),
                )
            }))
            .map_err(chart_error)?
            .label("Расходы")
            .legend(|(x, y)| Rectangle::new([(x, y - 7), (x + 24, y + 7)], EXPENSE_COLOR.filled()));

        chart
            .configure_series_labels()
            .position(SeriesLabelPosition::UpperLeft)
            .background_style(WHITE.mix(0.9))
            .border_style(BLACK)
            .label_font(("sans-serif", 22))
            .draw()
            .map_err(chart_error)?;

        root.present().map_err(chart_error)?;
    }

    let mut output = Cursor::new(Vec::new());
    let mut encoder = png::Encoder::new(&mut output, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::FilterType::Sub);
    encoder
        .write_header()
        .context("failed to create PNG header")?
        .write_image_data(&pixels)
        .context("failed to encode budget chart PNG")?;
    Ok(output.into_inner())
}

fn compact_money(value: i64) -> String {
    if value.abs() >= 1_000_000 {
        format!("{:.1} млн", value as f64 / 1_000_000.0)
    } else if value.abs() >= 1_000 {
        format!("{} тыс.", value / 1_000)
    } else {
        value.to_string()
    }
}

fn russian_month_label(value: &str) -> String {
    let Some((year, month)) = value.split_once('-') else {
        return value.to_string();
    };
    let month = match month {
        "01" => "январь",
        "02" => "февраль",
        "03" => "март",
        "04" => "апрель",
        "05" => "май",
        "06" => "июнь",
        "07" => "июль",
        "08" => "август",
        "09" => "сентябрь",
        "10" => "октябрь",
        "11" => "ноябрь",
        "12" => "декабрь",
        _ => return value.to_string(),
    };
    let short_year = year.get(year.len().saturating_sub(2)..).unwrap_or(year);
    format!("{month}, {short_year}")
}

fn chart_error<E: std::fmt::Debug>(error: E) -> anyhow::Error {
    anyhow::anyhow!("budget chart rendering failed: {error:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_png_with_two_series() {
        let png = render_budget_chart(&[
            BudgetMonth {
                label: "2026-08".into(),
                income: 100_000,
                expenses: 60_000,
            },
            BudgetMonth {
                label: "2026-09".into(),
                income: 130_000,
                expenses: 75_000,
            },
        ])
        .unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.len() > 10_000);
    }

    #[test]
    fn formats_russian_month_label() {
        assert_eq!(russian_month_label("2026-08"), "август, 26");
        assert_eq!(russian_month_label("2026-12"), "декабрь, 26");
    }
}
