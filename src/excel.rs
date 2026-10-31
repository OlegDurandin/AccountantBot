use anyhow::{Context, Result};
use rust_xlsxwriter::{Color, ExcelDateTime, Format, FormatAlign, FormatBorder, Url, Workbook};

use crate::repository::ExportOperation;

pub fn render_operations_excel(rows: &[ExportOperation]) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    workbook.set_zip_compression_level(1);
    let worksheet = workbook
        .add_worksheet_with_constant_memory()
        .context("failed to create constant-memory Excel worksheet")?;
    worksheet.set_name("История операций")?;
    worksheet.set_screen_gridlines(false);
    worksheet.set_freeze_panes(3, 0)?;
    worksheet.set_default_row_height(20);

    let title_format = Format::new()
        .set_font_name("Arial")
        .set_font_size(14)
        .set_bold()
        .set_font_color(Color::RGB(0x1F_29_37))
        .set_align(FormatAlign::Left)
        .set_align(FormatAlign::VerticalCenter);
    let header_format = Format::new()
        .set_font_name("Arial")
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1F_4E_78))
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::White);
    let text_format = Format::new()
        .set_font_name("Arial")
        .set_align(FormatAlign::VerticalCenter);
    let date_format = Format::new()
        .set_font_name("Arial")
        .set_num_format("dd.mm.yyyy hh:mm")
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter);
    let money_format = Format::new()
        .set_font_name("Arial")
        .set_num_format("#,##0 \"₽\";[Red]-#,##0 \"₽\"")
        .set_align(FormatAlign::Right)
        .set_align(FormatAlign::VerticalCenter);

    worksheet.merge_range(0, 0, 0, 5, "История финансовых операций", &title_format)?;
    worksheet.set_row_height(0, 24)?;

    let headers = [
        "Кто добавил",
        "Дата",
        "Тип операции",
        "Приход",
        "Расход",
        "Остаточный баланс",
    ];
    for (column, header) in headers.into_iter().enumerate() {
        worksheet.write_string_with_format(2, column as u16, header, &header_format)?;
    }
    worksheet.set_row_height(2, 24)?;

    for (index, operation) in rows.iter().enumerate() {
        let row = index as u32 + 3;
        let profile_url = format!("https://vk.com/id{}", operation.author_id);
        let profile_text = format!("vk.com/id{}", operation.author_id);
        worksheet.write_url_with_format(
            row,
            0,
            Url::new(profile_url).set_text(profile_text),
            &text_format,
        )?;

        let datetime = ExcelDateTime::parse_from_str(&operation.created_at)
            .with_context(|| format!("invalid operation date: {}", operation.created_at))?;
        worksheet.write_datetime_with_format(row, 1, &datetime, &date_format)?;
        worksheet.write_string_with_format(row, 2, &operation.operation_type, &text_format)?;

        if let Some(income) = operation.income {
            worksheet.write_number_with_format(row, 3, income as f64, &money_format)?;
        } else {
            worksheet.write_blank(row, 3, &money_format)?;
        }
        if let Some(expense) = operation.expense {
            worksheet.write_number_with_format(row, 4, expense as f64, &money_format)?;
        } else {
            worksheet.write_blank(row, 4, &money_format)?;
        }
        worksheet.write_number_with_format(row, 5, operation.balance as f64, &money_format)?;
    }

    if rows.is_empty() {
        worksheet.write_string_with_format(3, 0, "Операций пока нет", &text_format)?;
    } else {
        worksheet.autofilter(2, 0, rows.len() as u32 + 2, 5)?;
    }

    for (column, width) in [20.0, 20.0, 34.0, 16.0, 16.0, 21.0].into_iter().enumerate() {
        worksheet.set_column_width(column as u16, width)?;
    }

    workbook
        .save_to_buffer()
        .context("failed to build Excel workbook")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_xlsx_with_operation_history() {
        let bytes = render_operations_excel(&[
            ExportOperation {
                author_id: 7,
                created_at: "2026-10-01 10:30:00".into(),
                operation_type: "Приход".into(),
                income: Some(50_000),
                expense: None,
                balance: 50_000,
            },
            ExportOperation {
                author_id: 8,
                created_at: "2026-10-02 11:45:00".into(),
                operation_type: "Расход — Материалы".into(),
                income: None,
                expense: Some(4_000),
                balance: 46_000,
            },
        ])
        .unwrap();

        assert!(bytes.starts_with(b"PK"));
        assert!(bytes.len() > 5_000);
    }
}
