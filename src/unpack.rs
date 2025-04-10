use crate::Run;
use crate::constants::{IGNORE_LIST, tick_speed};
use crate::error::PDFConError;
use crate::pdf_image::{self, PDFConColorSpace};
use crate::progress::{bar, close_bar, spinner, update_end_cap};
use indicatif::ParallelProgressIterator;
use log::{debug, error};
use lopdf::{Dictionary, Document, Object};
use rayon::prelude::*;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpack {
    pub threads: usize,
    pub out_directory: PathBuf,
    pub in_file: PathBuf,
    pub optimize: bool,
}

pub fn filter_func(object_id: (u32, u16), object: &mut Object) -> Option<((u32, u16), Object)> {
    if IGNORE_LIST.contains(&object.type_name().unwrap_or_default()) {
        return None;
    }

    if let Ok(d) = object.as_dict_mut() {
        d.remove(b"Produce");
        d.remove(b"ModDate");
        d.remove(b"Creator");
        d.remove(b"ProcSet");
        d.remove(b"Procset");
        d.remove(b"MediaBox");
        d.remove(b"Annots");
        if d.is_empty() {
            return None;
        }
    }

    Some((object_id, object.to_owned()))
}

impl Unpack {
    fn process_xobject(
        &self,
        doc: &Document,
        page_num: u32,
        total_pages: usize,
        reference: &Object,
    ) -> Result<(), PDFConError> {
        debug!("Getting xobject information");
        let ref_id = reference.as_reference()?;

        debug!("Extracting stream");
        let stream = doc.get_object(ref_id)?.as_stream()?;

        debug!("Extracting subtype");
        let subtype = stream.dict.get(b"Subtype")?.as_name()?;

        debug!("Checking image");
        if subtype != b"Image" {
            // Not an image. No need to continue
            return Ok(());
        }

        debug!("Grabbing filter");
        let filters = match stream.dict.get(b"Filter") {
            Ok(f) => {
                let first = f.as_name();
                if first.is_ok() {
                    Some(vec![first.unwrap()])
                } else {
                    let second = f.as_str();
                    if second.is_ok() {
                        Some(vec![second.unwrap()])
                    } else {
                        let mut vec = Vec::new();
                        for filter in f.as_array()? {
                            vec.push(filter.as_name()?);
                        }
                        Some(vec)
                    }
                }
            }
            Err(_) => None,
        };

        match filters {
            Some(filter_list) => {
                // Filters are applied in reverse order from how they appear so
                // we're going to reverse this and apply the filters as the appear.
                // DCTDecode means this is a jpeg so we'll treat it as a jpeg. If DCT isn't present and only FlateDecode is
                // present then that means we're likely dealing with a png and we'll treat it as a png.
                // If no filter is present then that means some pdf builder sharted out raw pixel data into the
                // document. They shouldn't do this ( ImageMagick ) but we probably aught to handle this it.
                let mut is_jpeg = false;
                // I'd prefer not to clone but we may have to do that here. We should see if it's possible not to
                // duplicate the stream contents to process it
                let mut content = stream.content.clone();
                for filter in filter_list.into_iter().rev() {
                    if filter == b"DCTDecode" {
                        is_jpeg = true;
                    } else if filter == b"FlateDecode" {
                        content = pdf_image::decompress(&content)?;
                    }
                }

                // Calculate needed zero padding for page names
                let padding_width = (total_pages.ilog10() + 1) as usize;
                let path = self.out_directory.join(format!(
                    "{:0width$}.{}",
                    page_num,
                    if is_jpeg { "jpg" } else { "png" },
                    width = padding_width
                ));

                if is_jpeg {
                    pdf_image::save_jpeg(&content, &path, self.optimize)?
                } else {
                    let width = stream.dict.get(b"Width")?.as_i64()? as u32;
                    let height = stream.dict.get(b"Height")?.as_i64()? as u32;
                    let bits = stream.dict.get(b"BitsPerComponent")?.as_i64()? as u8;
                    let color_enum = PDFConColorSpace::from_pdf_format((
                        stream.dict.get(b"ColorSpace")?.as_name()?,
                        bits,
                    ));

                    pdf_image::encode_and_save_png(
                        &content,
                        width,
                        height,
                        &color_enum,
                        &path,
                        self.optimize,
                    )?
                }
            }
            None => {
                // This is a raw pixel buffer. We can encode this in any format we'd like
                // Treat it like its a png
                debug!("Raw pixel buffer");
                let width = stream.dict.get(b"Width")?.as_i64()? as u32;
                let height = stream.dict.get(b"Height")?.as_i64()? as u32;
                let bits = stream.dict.get(b"BitsPerComponent")?.as_i64()? as u8;
                let color_enum = PDFConColorSpace::from_pdf_format((
                    stream.dict.get(b"ColorSpace")?.as_name()?,
                    bits,
                ));

                let path = self.out_directory.join(format!("{:0>5}.png", page_num));

                pdf_image::encode_and_save_png(
                    &stream.content,
                    width,
                    height,
                    &color_enum,
                    &path,
                    self.optimize,
                )?
            }
        }

        Ok(())
    }

    fn find_xobject_images_in_page(
        &self,
        doc: &Document,
        page_num: u32,
        page_dict: &Dictionary,
        total_pages: usize,
    ) -> Result<(), PDFConError> {
        debug!("Getting resources and xobjects");
        let resources_dict = page_dict.get(b"Resources")?.as_dict()?;
        let x_obj_dict = resources_dict.get(b"XObject")?.as_dict()?;
        for (_name, x_ref) in x_obj_dict.iter() {
            self.process_xobject(&doc, page_num, total_pages, &x_ref)?;
        }
        Ok(())
    }

    fn extract_images(&self, doc: &Document) -> Result<(), PDFConError> {
        let pages = doc.get_pages().into_par_iter().collect::<Vec<_>>();
        let total_pages = pages.len();

        // Initialize the progress bar
        let pb = bar("Processing Images", total_pages as u64, tick_speed());

        let results: Vec<Result<(), PDFConError>> = pages
            .par_iter()
            .progress_with(pb.clone())
            .map(|(page_num, page_id)| {
                let pos = pb.position();
                let total = pb.length().unwrap();

                // Update bars end cap based on current progress
                update_end_cap(&pb, pos, total);

                debug!("Getting page dict");
                let page_dict = doc.get_object(*page_id)?.as_dict()?;
                self.find_xobject_images_in_page(&doc, *page_num, &page_dict, total_pages)?;
                Ok(())
            })
            .collect();

        // Finish bar and display message
        close_bar(pb, " ● Processing Complete! ");

        // Log any errors and return a general error
        let mut error_encountered = false;
        for result in results {
            match result {
                Ok(()) => {}
                Err(e) => {
                    error_encountered = true;
                    error!("Failed to extract image from page: {{{}}}", e.to_string())
                }
            }
        }
        if error_encountered {
            return Err(PDFConError::UnpackError);
        }
        Ok(())
    }
}

impl Run for Unpack {
    fn run(&self) -> Result<(), PDFConError> {
        rayon::ThreadPoolBuilder::new()
            .num_threads(self.threads)
            .build_global()?;

        std::fs::create_dir_all(&self.out_directory)?;

        // Add spinner to show program is doing something
        let spnr = spinner("Parsing PDF", tick_speed());

        let document = Document::load_filtered(&self.in_file, filter_func)?;

        // Finish bar and display message
        close_bar(spnr, " ● Parsing Complete! ");

        self.extract_images(&document)?;

        Ok(())
    }
}
